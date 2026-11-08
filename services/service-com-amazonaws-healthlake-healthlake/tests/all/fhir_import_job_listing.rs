// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `ListFHIRImportJobs`: what the service reports for
//! the FHIR import jobs a data store holds.
//!
//! The rule under test: `ListFHIRImportJobs` reports the import job records the
//! data store its required `DatastoreId` names holds — every one of them and
//! nothing belonging to another data store — narrowed by each filter member the
//! request sets, in one stable total order, split into pages of at most
//! `MaxResults` entries that `NextToken` resumes exactly; a request naming a
//! data store the service does not hold is refused with the modelled
//! `ResourceNotFoundException`, never with an empty page.
//!
//! `StartFHIRImportJob` — the operation that MINTS an import job record — needs
//! an input location in the emulator's Amazon S3 to read, so the rows below are
//! seeded directly into the SAME store this handler reads, through the shared
//! engine the harness exposes (exactly how `tests/all/fhir_export_job_listing.rs`
//! seeds export jobs and `tests/all/fhir_import_job_describe.rs` seeds import
//! jobs for the describe path); what a STARTED job records is pinned by
//! `tests/all/fhir_import_job_start.rs`. What ONE import job reports, member by member, is pinned by
//! `tests/all/fhir_import_job_describe.rs`; this suite pins what a LISTING of
//! them reports.
//! Every `ListFHIRImportJobs` call itself goes over the real awsJson1_0 wire
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
    ComAmazonawsHealthlakeHealthlakeState, DatastoreStatus, ImportJob, JobStatus,
    DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The epoch second the seeded submit instants are measured from. A fixed
/// instant, so a row's `SubmittedBefore` / `SubmittedAfter` cutoff is exactly
/// the instant a seeded job carries rather than something near it.
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
/// catalog by [`every_modelled_job_status_is_selectable`], so a status the model
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

/// One import job to seed: everything a row needs to be told apart from the
/// others in a listing.
struct Seed {
    id: &'static str,
    name: Option<&'static str>,
    status: JobStatus,
    /// The submit instant, as seconds after [`BASE_SECS`].
    submitted_after_base: i64,
    /// The transition the clock owes the seeded job, if any.
    pending: Option<JobStatus>,
}

impl Seed {
    fn new(id: &'static str, name: Option<&'static str>, status: JobStatus) -> Self {
        Self {
            id,
            name,
            status,
            submitted_after_base: 0,
            pending: None,
        }
    }

    fn submitted(mut self, submitted_after_base: i64) -> Self {
        self.submitted_after_base = submitted_after_base;
        self
    }

    fn pending(mut self, next: JobStatus) -> Self {
        self.pending = Some(next);
        self
    }
}

/// The `InputDataConfig` a seeded job carries: the modelled union's `S3Uri`
/// member, whose value satisfies the modelled `S3Uri` pattern
/// (`^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`).
fn input_s3_uri(job_id: &str) -> String {
    format!("s3://healthlake-import-bucket/input/{job_id}")
}

/// The `JobOutputDataConfig` a seeded job carries: a modelled
/// `S3Configuration`, whose `S3Uri` and `KmsKeyId` satisfy their own modelled
/// patterns so the reported value is one the model would admit.
fn output_data_config(job_id: &str) -> OutputDataConfig {
    OutputDataConfig::S3Configuration(S3Configuration {
        s3_uri: format!("s3://healthlake-import-bucket/output/{job_id}"),
        kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
    })
}

/// Seed `seeds` as import jobs of `datastore_id` in the engine the service
/// reads, returning their ids in the order the listing must report them
/// (submit instant ascending, identifier breaking a tie).
fn seed_jobs(service: &HealthLake, datastore_id: &str, seeds: &[Seed]) -> Vec<String> {
    let armed_at = Instant::now();
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        for seed in seeds {
            store.import_jobs.insert(
                seed.id.to_string(),
                ImportJob {
                    id: seed.id.to_string(),
                    datastore_id: datastore_id.to_string(),
                    name: seed.name.map(str::to_string),
                    status: seed.status,
                    // A seeded transition carries no `Message`: these rows are
                    // about what a listing REPORTS, not about why a job ended.
                    pending: seed
                        .pending
                        .map(|next| (armed_at + JOB_TRANSITION, next, None)),
                    submitted_at: ::aws_smithy_types::DateTime::from_secs(
                        BASE_SECS + seed.submitted_after_base,
                    ),
                    ended_at: None,
                    input_data_config: InputDataConfig::S3Uri(input_s3_uri(seed.id)),
                    output_data_config: Some(output_data_config(seed.id)),
                    data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                    message: None,
                    progress_report: None,
                    validation_level: None,
                    // A record this suite seeds was not started through the API,
                    // so it carries no `ClientToken` and there is no request to
                    // fingerprint.
                    client_token: None,
                    request_fingerprint: String::new(),
                },
            );
        }
    }
    let mut ordered: Vec<&Seed> = seeds.iter().collect();
    ordered.sort_by(|a, b| {
        a.submitted_after_base
            .cmp(&b.submitted_after_base)
            .then_with(|| a.id.cmp(b.id))
    });
    ordered.iter().map(|seed| seed.id.to_string()).collect()
}

/// The `JobId`s of one `ListFHIRImportJobs` response, in the order it reported
/// them.
fn job_ids(payload: &Value) -> Vec<String> {
    payload["ImportJobPropertiesList"]
        .as_array()
        .unwrap_or_else(|| panic!("ImportJobPropertiesList must be a list, got {payload}"))
        .iter()
        .map(|job| {
            job["JobId"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry must carry a JobId, got {job}"))
                .to_string()
        })
        .collect()
}

/// One listed job by `JobId`.
fn listed(payload: &Value, job_id: &str) -> Value {
    payload["ImportJobPropertiesList"]
        .as_array()
        .unwrap_or_else(|| panic!("ImportJobPropertiesList must be a list, got {payload}"))
        .iter()
        .find(|job| job["JobId"].as_str() == Some(job_id))
        .cloned()
        .unwrap_or_else(|| panic!("the listing must carry {job_id}, got {payload}"))
}

/// Create a data store and return its `DatastoreId`.
async fn datastore(service: &HealthLake, name: &str) -> String {
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }))
        .await;
    id
}

/// List every import job of `datastore_id`, with no filter and no paging.
async fn list_all(service: &HealthLake, datastore_id: &str) -> Value {
    service
        .ok(
            "ListFHIRImportJobs",
            &json!({ "DatastoreId": datastore_id }),
        )
        .await
}

#[tokio::test]
async fn every_job_of_the_named_data_store_is_reported_in_submit_order() {
    let service = HealthLake::new();
    let importing = datastore(&service, "importing store").await;
    let other = datastore(&service, "other store").await;

    // Seeded out of submit order, so a listing in insertion order would fail.
    let expected = seed_jobs(
        &service,
        &importing,
        &[
            Seed::new("job-charlie", Some("nightly"), JobStatus::Completed).submitted(30),
            Seed::new("job-alpha", Some("hourly"), JobStatus::Submitted).submitted(10),
            Seed::new("job-bravo", None, JobStatus::InProgress).submitted(20),
        ],
    );
    let elsewhere = seed_jobs(
        &service,
        &other,
        &[Seed::new("job-delta", Some("hourly"), JobStatus::Failed).submitted(5)],
    );

    let payload = list_all(&service, &importing).await;
    assert_eq!(
        job_ids(&payload),
        expected,
        "the listing must report every job of the named data store in submit order, got {payload}"
    );
    assert_eq!(
        job_ids(&list_all(&service, &other).await),
        elsewhere,
        "a data store's listing must report only its OWN jobs"
    );
    assert!(
        payload.get("NextToken").is_none(),
        "a single complete page must carry no NextToken, got {payload}"
    );

    // Every member the record holds is reported, under the modelled shape.
    let alpha = listed(&payload, "job-alpha");
    for (member, expected_value) in [
        ("JobId", json!("job-alpha")),
        ("JobName", json!("hourly")),
        ("JobStatus", json!("SUBMITTED")),
        ("DatastoreId", json!(importing)),
        ("DataAccessRoleArn", json!(DATA_ACCESS_ROLE_ARN)),
        (
            "InputDataConfig",
            json!({ "S3Uri": input_s3_uri("job-alpha") }),
        ),
        (
            "JobOutputDataConfig",
            json!({
                "S3Configuration": {
                    "S3Uri": "s3://healthlake-import-bucket/output/job-alpha",
                    "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                }
            }),
        ),
    ] {
        assert_eq!(
            alpha[member], expected_value,
            "case {member:?}: the listed job must report {expected_value}, got {alpha}"
        );
    }
    // A timestamp is carried as epoch seconds on this protocol, so the submit
    // instant is read as a number rather than compared to a JSON literal.
    let submit_time = alpha["SubmitTime"].as_f64();
    #[allow(clippy::cast_precision_loss)]
    let expected_submit_time = (BASE_SECS + 10) as f64;
    assert_eq!(
        submit_time,
        Some(expected_submit_time),
        "the listed job must report the instant it was submitted, got {submit_time:?}"
    );
    assert!(
        alpha.get("EndTime").is_none(),
        "a job that has not ended must report no EndTime, got {alpha}"
    );
    let unnamed = listed(&payload, "job-bravo");
    assert!(
        unnamed.get("JobName").is_none(),
        "a job started without a JobName must report none, got {unnamed}"
    );
}

#[tokio::test]
async fn every_filter_member_narrows_the_listing_to_the_jobs_it_names() {
    let service = HealthLake::new();
    let importing = datastore(&service, "filtered store").await;
    let other = datastore(&service, "other store").await;
    seed_jobs(
        &service,
        &importing,
        &[
            Seed::new("job-alpha", Some("hourly"), JobStatus::Submitted).submitted(10),
            Seed::new("job-bravo", Some("nightly"), JobStatus::InProgress).submitted(20),
            Seed::new("job-charlie", Some("nightly"), JobStatus::Completed).submitted(30),
            Seed::new("job-delta", None, JobStatus::CompletedWithErrors).submitted(40),
        ],
    );
    // A job of ANOTHER data store that every filter below would otherwise
    // select, so scoping is asserted by each row rather than once.
    seed_jobs(
        &service,
        &other,
        &[Seed::new("job-echo", Some("hourly"), JobStatus::Submitted).submitted(10)],
    );

    // The rows differ along which filter members are set, the form of the value
    // (a name held by one job, by two, by none), the status (drawn from the
    // modelled `JobStatus` catalog), and where a cutoff falls relative to a
    // submit instant: on it, one second inside it, one second outside it.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter member set",
            json!({}),
            vec!["job-alpha", "job-bravo", "job-charlie", "job-delta"],
        ),
        (
            "JobName held by exactly one job",
            json!({ "JobName": "hourly" }),
            vec!["job-alpha"],
        ),
        (
            "JobName held by two jobs",
            json!({ "JobName": "nightly" }),
            vec!["job-bravo", "job-charlie"],
        ),
        (
            "JobName held by no job of this data store",
            json!({ "JobName": "weekly" }),
            vec![],
        ),
        (
            "JobStatus held by one job",
            json!({ "JobStatus": "IN_PROGRESS" }),
            vec!["job-bravo"],
        ),
        (
            "JobStatus held by no job of this data store",
            json!({ "JobStatus": "CANCEL_IN_PROGRESS" }),
            vec![],
        ),
        (
            "SubmittedAfter exactly on a submit instant excludes that job",
            json!({ "SubmittedAfter": BASE_SECS + 30 }),
            vec!["job-delta"],
        ),
        (
            "SubmittedAfter one second before a submit instant admits it",
            json!({ "SubmittedAfter": BASE_SECS + 29 }),
            vec!["job-charlie", "job-delta"],
        ),
        (
            "SubmittedAfter one second past a submit instant excludes it",
            json!({ "SubmittedAfter": BASE_SECS + 31 }),
            vec!["job-delta"],
        ),
        (
            "SubmittedBefore exactly on a submit instant excludes that job",
            json!({ "SubmittedBefore": BASE_SECS + 20 }),
            vec!["job-alpha"],
        ),
        (
            "SubmittedBefore one second past a submit instant admits it",
            json!({ "SubmittedBefore": BASE_SECS + 21 }),
            vec!["job-alpha", "job-bravo"],
        ),
        (
            "SubmittedBefore one second before a submit instant excludes it",
            json!({ "SubmittedBefore": BASE_SECS + 19 }),
            vec!["job-alpha"],
        ),
        (
            "a window spanning the middle two jobs",
            json!({ "SubmittedAfter": BASE_SECS + 15, "SubmittedBefore": BASE_SECS + 35 }),
            vec!["job-bravo", "job-charlie"],
        ),
        (
            "a window whose own bounds are two submit instants excludes both",
            json!({ "SubmittedAfter": BASE_SECS + 20, "SubmittedBefore": BASE_SECS + 30 }),
            vec![],
        ),
        (
            "an empty window",
            json!({ "SubmittedAfter": BASE_SECS + 30, "SubmittedBefore": BASE_SECS + 20 }),
            vec![],
        ),
        (
            "JobName and JobStatus agreeing",
            json!({ "JobName": "nightly", "JobStatus": "COMPLETED" }),
            vec!["job-charlie"],
        ),
        (
            "JobName and JobStatus disagreeing",
            json!({ "JobName": "hourly", "JobStatus": "COMPLETED" }),
            vec![],
        ),
        (
            "every filter member at once",
            json!({
                "JobName": "nightly",
                "JobStatus": "IN_PROGRESS",
                "SubmittedAfter": BASE_SECS + 19,
                "SubmittedBefore": BASE_SECS + 21,
            }),
            vec!["job-bravo"],
        ),
    ];

    for (case, filter, expected) in &rows {
        let mut request = json!({ "DatastoreId": importing });
        for (member, value) in filter.as_object().expect("the filter row is an object") {
            request[member] = value.clone();
        }
        let payload = service.ok("ListFHIRImportJobs", &request).await;
        let listed_ids = job_ids(&payload);
        assert_eq!(
            listed_ids, *expected,
            "case {case:?}: the listing must report {expected:?}, got {listed_ids:?}"
        );
        assert!(
            !listed_ids.iter().any(|id| id == "job-echo"),
            "case {case:?}: a job of another data store must never be listed, got {listed_ids:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_job_status_is_selectable() {
    // The catalog is the generated `JOB_STATUS_VALUES`, which is the modelled
    // `JobStatus` enum's own value set: the stored statuses must spell exactly
    // it, so a filter can name every value the model admits.
    let wire: Vec<&str> = EVERY_STATUS.iter().map(|status| status.as_wire()).collect();
    assert_eq!(
        wire,
        JOB_STATUS_VALUES.to_vec(),
        "the stored statuses must spell the modelled JobStatus value set, got {wire:?}"
    );

    let service = HealthLake::new();
    let importing = datastore(&service, "every status").await;
    let seeds: Vec<Seed> = EVERY_STATUS
        .iter()
        .enumerate()
        .map(|(index, status)| {
            // The job's identifier is the status it holds, so a row names
            // itself in a failure message.
            Seed::new(status.as_wire(), None, *status)
                .submitted(i64::try_from(index).unwrap_or_default())
        })
        .collect();
    let expected = seed_jobs(&service, &importing, &seeds);
    assert_eq!(
        job_ids(&list_all(&service, &importing).await),
        expected,
        "an unfiltered listing must report one job per modelled status"
    );

    for status in &EVERY_STATUS {
        let case = status.as_wire();
        let payload = service
            .ok(
                "ListFHIRImportJobs",
                &json!({ "DatastoreId": importing, "JobStatus": case }),
            )
            .await;
        let listed_ids = job_ids(&payload);
        assert_eq!(
            listed_ids,
            vec![case.to_string()],
            "case {case:?}: filtering on a status must report exactly the job holding it, got {listed_ids:?}"
        );
    }
}

/// The identifiers the paging walk seeds, one per entry. Written out rather
/// than formatted so each is a `JobId` the modelled pattern admits.
const PAGED_IDS: [&str; 7] = [
    "job-0", "job-1", "job-2", "job-3", "job-4", "job-5", "job-6",
];

#[tokio::test]
async fn a_page_resumes_exactly_where_its_token_left_off() {
    let service = HealthLake::new();
    let importing = datastore(&service, "paged store").await;
    let seeds: Vec<Seed> = PAGED_IDS
        .iter()
        .enumerate()
        .map(|(index, id)| {
            Seed::new(id, None, JobStatus::Completed)
                .submitted(i64::try_from(index).unwrap_or_default())
        })
        .collect();
    let expected = seed_jobs(&service, &importing, &seeds);

    // The page sizes span the published `MaxResultsInteger` range (1..500): its
    // lower bound, sizes that do and do not divide the seeded count, the exact
    // count, one past it and the upper bound.
    for page_size in [1usize, 2, 3, 6, 7, 8, 500] {
        let case = format!("MaxResults={page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({ "DatastoreId": importing, "MaxResults": page_size });
            if let Some(resume) = &token {
                request["NextToken"] = json!(resume);
            }
            let payload = service.ok("ListFHIRImportJobs", &request).await;
            let page = job_ids(&payload);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: a page must hold 1..={page_size} entries, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len(),
                "case {case:?}: paging must terminate, got {pages} pages for {} entries",
                expected.len()
            );
            match payload.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert_eq!(
                        next,
                        expected[collected.len()],
                        "case {case:?}: NextToken must name the next entry in order, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages together must be the whole listing exactly once"
        );
    }

    // A page size outside the modelled 1..500 range is refused by the boundary
    // validator rather than clamped.
    for page_size in [0, 501] {
        let case = format!("MaxResults={page_size}");
        let (status, payload) = service
            .call(
                "ListFHIRImportJobs",
                &json!({ "DatastoreId": importing, "MaxResults": page_size }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside 1..500 must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_next_token_that_resumes_nothing_is_refused() {
    let service = HealthLake::new();
    let importing = datastore(&service, "token store").await;
    seed_jobs(
        &service,
        &importing,
        &[Seed::new("job-alpha", None, JobStatus::Completed)],
    );

    for (case, token) in [
        ("a token naming no job at all", "job-nothing"),
        ("the identifier of a job the filter excludes", "job-alpha"),
    ] {
        let (status, payload) = service
            .call(
                "ListFHIRImportJobs",
                &json!({
                    "DatastoreId": importing,
                    "NextToken": token,
                    "JobStatus": "FAILED",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unresumable NextToken must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn an_unknown_data_store_is_refused_rather_than_answered_with_an_empty_page() {
    let service = HealthLake::new();
    let importing = datastore(&service, "live store").await;
    seed_jobs(
        &service,
        &importing,
        &[Seed::new("job-alpha", None, JobStatus::Completed)],
    );

    for (case, datastore_id) in [
        (
            "a well-formed id nothing was minted under",
            "00000000000000000000000000000000".to_string(),
        ),
        ("an id of another shape entirely", "not-a-store".to_string()),
    ] {
        let (status, payload) = service
            .call(
                "ListFHIRImportJobs",
                &json!({ "DatastoreId": datastore_id }),
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
            !payload.to_string().contains(datastore_id.as_str()),
            "case {case:?}: a refusal must quote nothing from the request, got {payload}"
        );
    }

    // The live data store still lists its own job.
    assert_eq!(
        job_ids(&list_all(&service, &importing).await),
        vec!["job-alpha".to_string()],
        "the refusals above must leave the held records untouched"
    );
}

#[tokio::test]
async fn a_status_transition_the_clock_owes_is_reported_once_it_falls_due() {
    let service = HealthLake::new();
    let importing = datastore(&service, "settling store").await;
    seed_jobs(
        &service,
        &importing,
        &[
            Seed::new("job-moving", None, JobStatus::Submitted)
                .submitted(10)
                .pending(JobStatus::InProgress),
            Seed::new("job-ending", None, JobStatus::InProgress)
                .submitted(20)
                .pending(JobStatus::Completed),
            Seed::new("job-settled", None, JobStatus::Queued).submitted(30),
        ],
    );

    // Before the span is up every job still reports the status it was seeded
    // with, so a caller listing straight after a job starts sees the transient
    // one rather than the settled one.
    let payload = list_all(&service, &importing).await;
    for (case, expected) in [
        ("job-moving", "SUBMITTED"),
        ("job-ending", "IN_PROGRESS"),
        ("job-settled", "QUEUED"),
    ] {
        let job = listed(&payload, case);
        assert_eq!(
            job["JobStatus"], json!(expected),
            "case {case:?}: before the transition falls due the listing must report {expected:?}, got {job}"
        );
        assert!(
            job.get("EndTime").is_none(),
            "case {case:?}: a job that has not ended must report no EndTime, got {job}"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    let payload = list_all(&service, &importing).await;
    // A job that moved to a NON-terminal status has still not ended; the one
    // that reached a terminal status reports the instant it did.
    for (case, expected, ends) in [
        ("job-moving", "IN_PROGRESS", false),
        ("job-ending", "COMPLETED", true),
        ("job-settled", "QUEUED", false),
    ] {
        let job = listed(&payload, case);
        assert_eq!(
            job["JobStatus"],
            json!(expected),
            "case {case:?}: once due the listing must report {expected:?}, got {job}"
        );
        assert_eq!(
            job.get("EndTime").is_some(),
            ends,
            "case {case:?}: EndTime must be reported for a terminal status only, got {job}"
        );
    }

    // `EndTime` is stored when the job ends, not re-derived per read, so a
    // second listing reports the same instant.
    let again = list_all(&service, &importing).await;
    assert_eq!(
        listed(&again, "job-ending")["EndTime"],
        listed(&payload, "job-ending")["EndTime"],
        "two listings of one ended job must report the same EndTime"
    );
}

#[tokio::test]
async fn a_listing_is_a_report_the_next_caller_sees_unchanged() {
    let service = HealthLake::new();
    let importing = datastore(&service, "reported store").await;
    let expected = seed_jobs(
        &service,
        &importing,
        &[
            Seed::new("job-alpha", Some("hourly"), JobStatus::Queued).submitted(10),
            Seed::new("job-bravo", None, JobStatus::Completed).submitted(20),
        ],
    );

    // Listing, filtering and paging are all reads: a second unfiltered listing
    // reports exactly what the first did, whatever was asked in between.
    let first = list_all(&service, &importing).await;
    for request in [
        json!({ "DatastoreId": importing, "JobStatus": "QUEUED" }),
        json!({ "DatastoreId": importing, "MaxResults": 1 }),
        json!({ "DatastoreId": importing, "JobName": "hourly" }),
    ] {
        service.ok("ListFHIRImportJobs", &request).await;
    }
    let again = list_all(&service, &importing).await;
    assert_eq!(
        job_ids(&again),
        expected,
        "a listing must leave every record where it was, got {again}"
    );
    assert_eq!(
        again, first,
        "two identical listings must report the same page"
    );
}

#[tokio::test]
async fn a_job_does_not_outlive_the_data_store_it_imported_into() {
    let service = HealthLake::new();
    let importing = datastore(&service, "reaped store").await;
    seed_jobs(
        &service,
        &importing,
        &[Seed::new("job-alpha", None, JobStatus::Completed)],
    );
    assert_eq!(
        job_ids(&list_all(&service, &importing).await),
        vec!["job-alpha".to_string()],
        "the seeded job must be listed while its data store is held"
    );

    // Put the data store at the end of its own retention, which is what a
    // delete leaves behind once the retention has run out.
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        let record = store
            .datastores
            .get_mut(&importing)
            .expect("the created data store is held");
        record.status = DatastoreStatus::Deleted;
        record.pending = None;
        record.reap_at = Some(Instant::now());
    }

    let (status, payload) = service
        .call(
            "ListFHIRImportJobs",
            &json!({ "DatastoreId": importing }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a reaped data store must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "a reaped data store must answer ResourceNotFoundException, got {payload}"
    );
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let held = store.lock().import_jobs.len();
    assert_eq!(
        held, 0,
        "the reaped data store's jobs must be dropped with it, {held} still held"
    );
}
