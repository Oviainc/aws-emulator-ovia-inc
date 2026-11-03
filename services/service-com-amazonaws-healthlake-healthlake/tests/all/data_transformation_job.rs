// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `DescribeDataTransformationJob`: what the service
//! reports for one data transformation job.
//!
//! The rule under test: `DescribeDataTransformationJob` reports the
//! transformation job record its required `JobId` names — every member that
//! record holds and nothing else, under the modelled
//! `TransformationJobProperties` — after letting the record catch up with the
//! clock, so a status transition already due is reported and one not yet due is
//! not; a `JobId` no job record holds is refused with the modelled
//! `ResourceNotFoundException` in this service's fixed not-found sentence,
//! never with an empty or synthesised job.
//!
//! `StartDataTransformationJob` — the operation that MINTS a job record — is
//! covered end-to-end by `tests/all/start_data_transformation_job.rs`; the rows
//! below are seeded directly into the SAME store this handler reads, through
//! the shared engine the harness exposes (exactly how
//! `tests/all/fhir_export_job_listing.rs` seeds export jobs), so one record can
//! differ from the next along any member a start does not choose. Every
//! `DescribeDataTransformationJob` call itself goes over the real
//! awsJson1_0 wire through `MultiServiceRouter::dispatch` — the entry point
//! `core/router` serves — so routing, deserialization, the modelled constraints
//! and the error envelope are all exercised, not bypassed.
//!
//! Every modelled CLIENT error below is asserted at HTTP 400, whatever exact
//! code its shape's `@httpError` trait names: awsJson1_0 binds nothing to the
//! transport, so this service reports an error's FAULT. The rule itself is
//! pinned in `tests/all/error_envelopes.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::ops::types::{
    DataTransformationS3Configuration, TransformationInputDataConfig,
    TransformationJobProgressReport, TransformationOutputDataConfig,
    TRANSFORMATION_JOB_STATUS_VALUES,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, ExportJob, JobStatus, TransformationJob,
    TransformationJobStatus,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The epoch second the seeded submit instants are measured from. A fixed
/// instant, so a row's reported `SubmitTime` is exactly the one it was seeded
/// with rather than something near it.
const BASE_SECS: i64 = 1_700_000_000;

/// The span a seeded clock-owed job transition falls due after it is armed.
///
/// Chosen to match `crate::state::LIFECYCLE_TRANSITION`, the span this
/// service's other pending transitions use, so one wait ([`PAST_TRANSITION`])
/// outlasts either.
const JOB_TRANSITION: Duration = Duration::from_millis(500);

/// Every status the modelled `TransformationJobStatus` enum declares, in the
/// order the generated `TRANSFORMATION_JOB_STATUS_VALUES` catalog lists them.
/// Pinned against that catalog by
/// [`every_modelled_transformation_job_status_is_reported`], so a status the
/// model gains cannot stay untested here.
const EVERY_STATUS: [TransformationJobStatus; 6] = [
    TransformationJobStatus::Completed,
    TransformationJobStatus::CompletedWithErrors,
    TransformationJobStatus::Failed,
    TransformationJobStatus::InProgress,
    TransformationJobStatus::Queued,
    TransformationJobStatus::Submitted,
];

/// The identifier each [`EVERY_STATUS`] row is seeded under, in the same
/// order: the status it holds, spelled with the `-` the modelled
/// `DataTransformationJobId` pattern (`^[a-zA-Z0-9-]+$`) admits in place of the
/// `_` it does not, so a row still names itself in a failure message.
const STATUS_JOB_IDS: [&str; 6] = [
    "job-completed",
    "job-completed-with-errors",
    "job-failed",
    "job-in-progress",
    "job-queued",
    "job-submitted",
];

/// The `DataAccessRoleArn` a seeded job carries, satisfying the modelled
/// `DataTransformationIamRoleArn` pattern
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeTransform";

/// One transformation job to seed: every member the modelled
/// `TransformationJobProperties` can carry, so a row can differ from the others
/// along any one of them.
#[derive(Clone)]
struct Seed {
    id: &'static str,
    status: TransformationJobStatus,
    /// The submit instant, as seconds after [`BASE_SECS`].
    submitted_after_base: i64,
    /// `InputDataConfig$S3Uri`, satisfying the modelled
    /// `DataTransformationS3Uri` pattern.
    input_s3_uri: &'static str,
    /// `InputDataConfig$SourceFormat`, from the modelled `SourceFormat` value
    /// set (`CCDA`, `CSV`), or `None` when the start call named none.
    source_format: Option<&'static str>,
    /// `OutputDataConfig$S3Configuration$S3Uri` and `$KmsKeyId`, both required
    /// members of the modelled `DataTransformationS3Configuration`.
    output_s3_uri: &'static str,
    kms_key_id: &'static str,
    name: Option<&'static str>,
    /// The profile the job runs against, as `(ProfileId, ProfileName,
    /// ProfileVersion)`; `None` for a job record holding none.
    profile: Option<(&'static str, &'static str, i32)>,
    drift_detection_enabled: Option<bool>,
    provenance_enabled: Option<bool>,
    message: Option<&'static str>,
    /// `JobProgressReport`, as `[TotalFilesScanned, TotalFilesConverted,
    /// TotalFilesFailed, TotalResourcesGenerated]`.
    progress: Option<[i64; 4]>,
    /// `EndTime`, as seconds after [`BASE_SECS`], for a job that has ended.
    ended_after_base: Option<i64>,
    /// The transition the clock owes the seeded job, if any.
    pending: Option<TransformationJobStatus>,
}

impl Seed {
    /// A job carrying only what the modelled shape REQUIRES: an identifier, a
    /// status, both data configurations, the access role and a submit instant.
    fn new(id: &'static str, status: TransformationJobStatus) -> Self {
        Self {
            id,
            status,
            submitted_after_base: 0,
            input_s3_uri: "s3://healthlake-transform-input/ccda",
            source_format: None,
            output_s3_uri: "s3://healthlake-transform-output/fhir",
            kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab",
            name: None,
            profile: None,
            drift_detection_enabled: None,
            provenance_enabled: None,
            message: None,
            progress: None,
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

    fn input(mut self, s3_uri: &'static str, source_format: Option<&'static str>) -> Self {
        self.input_s3_uri = s3_uri;
        self.source_format = source_format;
        self
    }

    fn output(mut self, s3_uri: &'static str, kms_key_id: &'static str) -> Self {
        self.output_s3_uri = s3_uri;
        self.kms_key_id = kms_key_id;
        self
    }

    fn profile(mut self, id: &'static str, name: &'static str, version: i32) -> Self {
        self.profile = Some((id, name, version));
        self
    }

    fn flags(mut self, drift: Option<bool>, provenance: Option<bool>) -> Self {
        self.drift_detection_enabled = drift;
        self.provenance_enabled = provenance;
        self
    }

    fn message(mut self, message: &'static str) -> Self {
        self.message = Some(message);
        self
    }

    fn progress(mut self, progress: [i64; 4]) -> Self {
        self.progress = Some(progress);
        self
    }

    fn ended(mut self, ended_after_base: i64) -> Self {
        self.ended_after_base = Some(ended_after_base);
        self
    }

    fn pending(mut self, next: TransformationJobStatus) -> Self {
        self.pending = Some(next);
        self
    }

    /// This row as the stored record the handler reads.
    fn record(&self, armed_at: Instant) -> TransformationJob {
        TransformationJob {
            id: self.id.to_string(),
            name: self.name.map(str::to_string),
            status: self.status,
            pending: self
                .pending
                .map(|next| (armed_at + JOB_TRANSITION, next, None)),
            submitted_at: ::aws_smithy_types::DateTime::from_secs(
                BASE_SECS + self.submitted_after_base,
            ),
            ended_at: self
                .ended_after_base
                .map(|offset| ::aws_smithy_types::DateTime::from_secs(BASE_SECS + offset)),
            input_data_config: TransformationInputDataConfig {
                s3_uri: self.input_s3_uri.to_string(),
                source_format: self.source_format.map(str::to_string),
            },
            output_data_config: TransformationOutputDataConfig {
                s3_configuration: DataTransformationS3Configuration {
                    s3_uri: self.output_s3_uri.to_string(),
                    kms_key_id: self.kms_key_id.to_string(),
                },
            },
            data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
            profile_id: self.profile.map(|(id, _, _)| id.to_string()),
            profile_name: self.profile.map(|(_, name, _)| name.to_string()),
            profile_version: self.profile.map(|(_, _, version)| version),
            drift_detection_enabled: self.drift_detection_enabled,
            provenance_enabled: self.provenance_enabled,
            message: self.message.map(str::to_string),
            progress_report: self.progress.map(|counts| TransformationJobProgressReport {
                total_files_scanned: counts[0],
                total_files_converted: counts[1],
                total_files_failed: counts[2],
                total_resources_generated: counts[3],
            }),
            // Seeded rather than started, so no `ClientToken` named this job.
            client_token: None,
        }
    }

    /// The `TransformationJobProperties` this row must be reported as, WITHOUT
    /// its two timestamp members: a timestamp is carried as epoch seconds on
    /// this protocol, so `SubmitTime` and `EndTime` are read as numbers rather
    /// than compared to a JSON literal.
    fn expected_properties(&self) -> Value {
        let mut expected = json!({
            "JobId": self.id,
            "JobStatus": self.status.as_wire(),
            "InputDataConfig": { "S3Uri": self.input_s3_uri },
            "OutputDataConfig": {
                "S3Configuration": {
                    "S3Uri": self.output_s3_uri,
                    "KmsKeyId": self.kms_key_id,
                }
            },
            "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
        });
        if let Some(source_format) = self.source_format {
            expected["InputDataConfig"]["SourceFormat"] = json!(source_format);
        }
        if let Some(name) = self.name {
            expected["JobName"] = json!(name);
        }
        if let Some((id, name, version)) = self.profile {
            expected["ProfileId"] = json!(id);
            expected["ProfileName"] = json!(name);
            expected["ProfileVersion"] = json!(version);
        }
        if let Some(drift) = self.drift_detection_enabled {
            expected["DriftDetectionEnabled"] = json!(drift);
        }
        if let Some(provenance) = self.provenance_enabled {
            expected["ProvenanceEnabled"] = json!(provenance);
        }
        if let Some(message) = self.message {
            expected["Message"] = json!(message);
        }
        if let Some(counts) = self.progress {
            expected["JobProgressReport"] = json!({
                "TotalFilesScanned": counts[0],
                "TotalFilesConverted": counts[1],
                "TotalFilesFailed": counts[2],
                "TotalResourcesGenerated": counts[3],
            });
        }
        expected
    }
}

/// Seed `seeds` as transformation job records in the engine the service reads.
fn seed_jobs(service: &HealthLake, seeds: &[Seed]) {
    let armed_at = Instant::now();
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    for seed in seeds {
        store
            .transformation_jobs
            .insert(seed.id.to_string(), seed.record(armed_at));
    }
}

/// The `TransformationJobProperties` `DescribeDataTransformationJob` reports
/// for `job_id`.
async fn describe(service: &HealthLake, job_id: &str) -> Value {
    let payload = service
        .ok("DescribeDataTransformationJob", &json!({ "JobId": job_id }))
        .await;
    payload["TransformationJobProperties"].clone()
}

/// `properties` without its timestamp members, beside the epoch seconds each
/// carried (`None` when the member was not reported at all).
fn split_timestamps(properties: &Value) -> (Value, Option<f64>, Option<f64>) {
    let mut rest = properties.clone();
    let object = rest.as_object_mut().unwrap_or_else(|| {
        panic!("TransformationJobProperties must be an object, got {properties}")
    });
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
/// which optional members are present, the form of each value (both modelled
/// `SourceFormat` values and none, a KMS key as a bare id and as an ARN, an
/// ASCII and a unicode `JobName`, a name at the modelled 64-character maximum),
/// the status, both published `ProfileVersion` bounds (0..99), and progress
/// counts of zero, of a handful and of a large run.
fn round_trip_rows() -> Vec<(&'static str, Seed)> {
    vec![
        (
            "only the required members",
            Seed::new("job-minimal", TransformationJobStatus::Submitted),
        ),
        (
            "named, CCDA source, DRAFT profile version",
            Seed::new("job-ccda", TransformationJobStatus::Queued)
                .submitted(10)
                .named("nightly-ccda")
                .input("s3://transform-in.ccda-bucket/incoming", Some("CCDA"))
                .profile("0123456789abcdef0123456789abcdef", "ccda-to-r4", 0),
        ),
        (
            "CSV source, highest published profile version, both flags set",
            Seed::new("job-csv", TransformationJobStatus::InProgress)
                .submitted(20)
                .named("hourly csv")
                .input("s3://transform-in/csv/batch-07", Some("CSV"))
                .output(
                    "s3://transform-out.fhir/r4",
                    "arn:aws:kms:us-east-1:111122223333:key/abcd1234",
                )
                .profile("fedcba9876543210fedcba9876543210", "csv-to-r4", 99)
                .flags(Some(true), Some(true))
                .progress([120, 60, 0, 1_430]),
        ),
        (
            "unicode JobName, flags explicitly disabled, no profile",
            Seed::new("job-unicode", TransformationJobStatus::InProgress)
                .submitted(30)
                .named("клиника записи 42")
                .flags(Some(false), Some(false))
                .progress([0, 0, 0, 0]),
        ),
        (
            "completed, JobName at the modelled 64-character maximum, large counts",
            Seed::new("job-completed", TransformationJobStatus::Completed)
                .submitted(40)
                .named("n.n:n/n=n+n-n%n@n_n n.n:n/n=n+n-n%n@n_n n.n:n/n=n+n-n%n@n_n nnnn")
                .profile("00112233445566778899aabbccddeeff", "wide-profile", 7)
                .progress([9_000, 9_000, 0, 1_234_567])
                .ended(45),
        ),
        (
            "completed with errors, a message and a failed-file count",
            Seed::new("job-partial", TransformationJobStatus::CompletedWithErrors)
                .submitted(50)
                .input("s3://transform-in/mixed", Some("CCDA"))
                .message("3 of 10 source files could not be converted")
                .progress([10, 7, 3, 84])
                .ended(55),
        ),
        (
            "failed, a message and nothing converted",
            Seed::new("job-failed", TransformationJobStatus::Failed)
                .submitted(60)
                .message("the data access role could not read the input location")
                .progress([4, 0, 4, 0])
                .ended(61),
        ),
    ]
}

#[tokio::test]
async fn every_member_of_a_held_job_is_reported_under_the_modelled_shape() {
    let service = HealthLake::new();
    let rows = round_trip_rows();
    seed_jobs(
        &service,
        &rows.iter().map(|(_, seed)| seed.clone()).collect::<Vec<_>>(),
    );

    for (case, seed) in &rows {
        let properties = describe(&service, seed.id).await;
        let (reported, submit_time, end_time) = split_timestamps(&properties);
        assert_eq!(
            reported,
            seed.expected_properties(),
            "case {case:?}: the described job must report exactly the members the record holds, got {properties}"
        );
        assert_eq!(
            submit_time,
            Some(epoch_secs(seed.submitted_after_base)),
            "case {case:?}: must report the instant the job was submitted, got {submit_time:?}"
        );
        assert_eq!(
            end_time,
            seed.ended_after_base.map(epoch_secs),
            "case {case:?}: EndTime must be reported exactly for a job that ended, got {end_time:?}"
        );
    }

    // Describing a job reports it; it never moves, ends or consumes it, so a
    // second call answers exactly the same thing.
    for (case, seed) in &rows {
        let again = describe(&service, seed.id).await;
        let first = split_timestamps(&describe(&service, seed.id).await);
        assert_eq!(
            split_timestamps(&again),
            first,
            "case {case:?}: two describes of one job must report the same properties"
        );
    }
}

#[tokio::test]
async fn every_modelled_transformation_job_status_is_reported() {
    // The catalog is the generated `TRANSFORMATION_JOB_STATUS_VALUES`, which is
    // the modelled `TransformationJobStatus` enum's own value set: the stored
    // statuses must spell exactly it, so no record can report a status the
    // model would refuse and none it admits is untested.
    let wire: Vec<&str> = EVERY_STATUS.iter().map(|status| status.as_wire()).collect();
    assert_eq!(
        wire,
        TRANSFORMATION_JOB_STATUS_VALUES.to_vec(),
        "the stored statuses must spell the modelled TransformationJobStatus value set, got {wire:?}"
    );

    let service = HealthLake::new();
    let seeds: Vec<Seed> = EVERY_STATUS
        .iter()
        .zip(STATUS_JOB_IDS)
        .enumerate()
        .map(|(index, (status, id))| {
            // The job's identifier names the status it holds, so a row names
            // itself in a failure message. A terminal status also carries the
            // `EndTime` it reached.
            let seed =
                Seed::new(id, *status).submitted(i64::try_from(index).unwrap_or_default());
            if status.is_terminal() {
                seed.ended(100)
            } else {
                seed
            }
        })
        .collect();
    seed_jobs(&service, &seeds);

    for (status, seed) in EVERY_STATUS.iter().zip(&seeds) {
        let case = status.as_wire();
        let properties = describe(&service, seed.id).await;
        assert_eq!(
            properties["JobStatus"],
            json!(case),
            "case {case:?}: must report the status the record holds, got {properties}"
        );
        let (_, _, end_time) = split_timestamps(&properties);
        assert_eq!(
            end_time.is_some(),
            status.is_terminal(),
            "case {case:?}: EndTime must be reported for a terminal status only, got {properties}"
        );
    }
}

#[tokio::test]
async fn a_job_id_no_record_holds_is_refused_with_the_fixed_not_found_sentence() {
    let service = HealthLake::new();
    seed_jobs(
        &service,
        &[Seed::new("job-held", TransformationJobStatus::Queued)],
    );
    // A FHIR export job's record lives in the same state under its own id; it
    // is not a transformation job, so its id must not resolve as one.
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        store.export_jobs.insert(
            "job-export".to_string(),
            ExportJob {
                id: "job-export".to_string(),
                datastore_id: "00000000000000000000000000000000".to_string(),
                name: None,
                status: JobStatus::Completed,
                pending: None,
                submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                ended_at: None,
                output_data_config:
                    service_com_amazonaws_healthlake_healthlake::ops::types::OutputDataConfig::S3Configuration(
                        service_com_amazonaws_healthlake_healthlake::ops::types::S3Configuration {
                            s3_uri: "s3://healthlake-export-bucket/job-export".to_string(),
                            kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
                        },
                    ),
                data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                message: None,
                // Seeded rather than started, so no `ClientToken` named this
                // job and no request fingerprinted to it.
                client_token: None,
                request_fingerprint: String::new(),
            },
        );
    }

    // The rows span the forms a `JobId` the modelled constraints admit can
    // take: one never minted, one of another shape entirely, the identifier of
    // a DIFFERENT resource kind held in the same state, and a case variant of a
    // held id (the identifier is case-sensitive).
    for (case, job_id) in [
        ("a well-formed id nothing was minted under", "job-nothing"),
        ("an id of another shape entirely", "0123456789"),
        ("the id of a FHIR export job", "job-export"),
        ("a held id in another case", "JOB-HELD"),
    ] {
        let (status, payload) = service
            .call("DescribeDataTransformationJob", &json!({ "JobId": job_id }))
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
            service_com_amazonaws_healthlake_healthlake::state::transformation_job_not_found_message(
                job_id
            ),
            "case {case:?}: must state the not-found sentence naming the job, got {payload}"
        );
        assert!(
            payload.to_string().contains(job_id),
            "case {case:?}: the refusal must name the job that was not found, got {payload}"
        );
    }

    // The refusals above left the held record readable.
    let properties = describe(&service, "job-held").await;
    assert_eq!(
        properties["JobId"],
        json!("job-held"),
        "the refusals must leave the held record untouched, got {properties}"
    );
}

#[tokio::test]
async fn a_job_id_outside_the_modelled_constraints_is_refused() {
    let service = HealthLake::new();
    seed_jobs(
        &service,
        &[Seed::new("job-held", TransformationJobStatus::Queued)],
    );

    // `DataTransformationJobId` is len 1..32, pattern `^[a-zA-Z0-9-]+$`: the
    // rows are each published bound and one past it, plus a character the
    // alphabet excludes.
    for (case, job_id) in [
        ("empty, one below the minimum length", String::new()),
        ("at the minimum length", "a".to_string()),
        ("one past the maximum length", "a".repeat(33)),
        ("an underscore the pattern excludes", "job_held".to_string()),
        ("a space the pattern excludes", "job held".to_string()),
    ] {
        let (status, payload) = service
            .call("DescribeDataTransformationJob", &json!({ "JobId": job_id }))
            .await;
        if case == "at the minimum length" {
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
}

#[tokio::test]
async fn a_status_transition_the_clock_owes_is_reported_once_it_falls_due() {
    let service = HealthLake::new();
    seed_jobs(
        &service,
        &[
            Seed::new("job-moving", TransformationJobStatus::Submitted)
                .submitted(10)
                .pending(TransformationJobStatus::InProgress),
            Seed::new("job-ending", TransformationJobStatus::InProgress)
                .submitted(20)
                .pending(TransformationJobStatus::Completed),
            Seed::new("job-settled", TransformationJobStatus::Queued).submitted(30),
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
            let properties = describe(&service, case).await;
            assert_eq!(
                properties["JobStatus"],
                json!(expected),
                "case {case:?} ({attempt}): before the transition falls due the job must report {expected:?}, got {properties}"
            );
            assert!(
                properties.get("EndTime").is_none(),
                "case {case:?} ({attempt}): a job that has not ended must report no EndTime, got {properties}"
            );
        }
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    // A job that moved to a NON-terminal status has still not ended; the one
    // that reached a terminal status reports the instant it did.
    for (case, expected, ends) in [
        ("job-moving", "IN_PROGRESS", false),
        ("job-ending", "COMPLETED", true),
        ("job-settled", "QUEUED", false),
    ] {
        let properties = describe(&service, case).await;
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
    }

    // `EndTime` is stored when the job ends, not re-derived per read, so a
    // second describe reports the same instant.
    let first = describe(&service, "job-ending").await;
    let again = describe(&service, "job-ending").await;
    assert_eq!(
        again["EndTime"], first["EndTime"],
        "two describes of one ended job must report the same EndTime, got {again} after {first}"
    );
}
