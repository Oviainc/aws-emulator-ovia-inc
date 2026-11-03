// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `ListDataTransformationJobs`: what the service
//! reports for the data transformation jobs this account holds.
//!
//! The rule under test: `ListDataTransformationJobs` reports the transformation
//! job records the service holds — every one of them, through the modelled
//! `TransformationJobSummary` and nothing beyond it — narrowed by each filter
//! member the request sets, in one stable total order, split into pages of at
//! most `MaxResults` entries that `NextToken` resumes exactly; a job belongs to
//! no data store, so a filter selecting nothing is an empty page rather than a
//! refusal.
//!
//! `StartDataTransformationJob` — the operation that MINTS a job record —
//! starts a job in one status and leaves the rest to the clock, so the rows
//! below are mostly seeded directly into the SAME store this handler reads,
//! through the shared engine the harness exposes (exactly how
//! `tests/all/data_transformation_job.rs` seeds the records
//! `DescribeDataTransformationJob` reports), to put a job in each status the
//! model declares without waiting for one. What a STARTED job reports is pinned
//! by `tests/all/start_data_transformation_job.rs`, and
//! [`a_started_job_is_listed_exactly_as_describe_reports_it`] below ties the two
//! readers to the one record.
//!
//! Every `ListDataTransformationJobs` call itself goes over the real awsJson1_0
//! wire through `MultiServiceRouter::dispatch` — the entry point `core/router`
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
    DataTransformationS3Configuration, TransformationInputDataConfig,
    TransformationOutputDataConfig, SOURCE_FORMAT_VALUES, TRANSFORMATION_JOB_STATUS_VALUES,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, TransformationJob, TransformationJobStatus,
};

use crate::fhir_datastore_lifecycle::{error_code, string_at, HealthLake, PAST_TRANSITION};

/// The epoch second the seeded submit instants are measured from. A fixed
/// instant, so a row's `SubmittedBefore` / `SubmittedAfter` cutoff is exactly
/// the instant a seeded job carries rather than something near it.
const BASE_SECS: i64 = 1_700_000_000;

/// The span a seeded clock-owed job transition falls due after it is armed.
///
/// Chosen to match `crate::state::TRANSFORMATION_JOB_RUN`, the span this
/// service's own pending job transitions use, so one wait
/// ([`PAST_TRANSITION`]) outlasts either.
const JOB_TRANSITION: Duration = Duration::from_millis(500);

/// The `DataAccessRoleArn` every job here carries, satisfying the modelled
/// `DataTransformationIamRoleArn` pattern
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeTransform";

/// Every status the modelled `TransformationJobStatus` enum declares, in the
/// order the generated `TRANSFORMATION_JOB_STATUS_VALUES` catalog lists them.
/// Pinned against that catalog by
/// [`every_modelled_transformation_job_status_is_selectable`], so a status the
/// model gains cannot stay untested here.
const EVERY_STATUS: [TransformationJobStatus; 6] = [
    TransformationJobStatus::Completed,
    TransformationJobStatus::CompletedWithErrors,
    TransformationJobStatus::Failed,
    TransformationJobStatus::InProgress,
    TransformationJobStatus::Queued,
    TransformationJobStatus::Submitted,
];

/// Exactly the members the modelled `TransformationJobSummary` declares, so a
/// listing reporting anything else — or carrying a member the shape does not
/// have — fails here.
const SUMMARY_MEMBERS: [&str; 6] = [
    "JobId",
    "JobStatus",
    "SubmitTime",
    "JobName",
    "EndTime",
    "SourceFormat",
];

/// One transformation job to seed: the members a summary reports, so a row can
/// differ from the others along any one of them.
#[derive(Clone)]
struct Seed {
    id: &'static str,
    name: Option<&'static str>,
    status: TransformationJobStatus,
    /// The submit instant, as seconds after [`BASE_SECS`].
    submitted_after_base: i64,
    /// `EndTime`, as seconds after [`BASE_SECS`], for a job that has ended.
    ended_after_base: Option<i64>,
    /// `InputDataConfig$SourceFormat`, from the modelled `SourceFormat` value
    /// set, or `None` when the start call named none.
    source_format: Option<&'static str>,
    /// The transition the clock owes the seeded job, if any.
    pending: Option<TransformationJobStatus>,
}

impl Seed {
    fn new(id: &'static str, name: Option<&'static str>, status: TransformationJobStatus) -> Self {
        Self {
            id,
            name,
            status,
            submitted_after_base: 0,
            ended_after_base: None,
            source_format: None,
            pending: None,
        }
    }

    fn submitted(mut self, submitted_after_base: i64) -> Self {
        self.submitted_after_base = submitted_after_base;
        self
    }

    fn ended(mut self, ended_after_base: i64) -> Self {
        self.ended_after_base = Some(ended_after_base);
        self
    }

    fn source_format(mut self, source_format: &'static str) -> Self {
        self.source_format = Some(source_format);
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
            // A seeded transition carries no `Message`: these rows are about
            // what a LISTING reports, and the summary shape declares no
            // message at all.
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
                s3_uri: "s3://healthlake-transform-input/ccda".to_string(),
                source_format: self.source_format.map(str::to_string),
            },
            output_data_config: TransformationOutputDataConfig {
                s3_configuration: DataTransformationS3Configuration {
                    s3_uri: "s3://healthlake-transform-output/fhir".to_string(),
                    kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
                },
            },
            data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
            profile_id: None,
            profile_name: None,
            profile_version: None,
            drift_detection_enabled: None,
            provenance_enabled: None,
            message: None,
            progress_report: None,
            // Seeded rather than started, so no `ClientToken` named this job.
            client_token: None,
        }
    }
}

/// Seed `seeds` as transformation job records in the engine the service reads,
/// returning their ids in the order the listing must report them (submit
/// instant ascending, identifier breaking a tie).
fn seed_jobs(service: &HealthLake, seeds: &[Seed]) -> Vec<String> {
    let armed_at = Instant::now();
    {
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
    let mut ordered: Vec<&Seed> = seeds.iter().collect();
    ordered.sort_by(|a, b| {
        a.submitted_after_base
            .cmp(&b.submitted_after_base)
            .then_with(|| a.id.cmp(b.id))
    });
    ordered.iter().map(|seed| seed.id.to_string()).collect()
}

/// The `JobId`s of one `ListDataTransformationJobs` response, in the order it
/// reported them.
fn job_ids(payload: &Value) -> Vec<String> {
    items(payload)
        .iter()
        .map(|job| {
            job["JobId"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry must carry a JobId, got {job}"))
                .to_string()
        })
        .collect()
}

/// The `Items` list of one response.
fn items(payload: &Value) -> Vec<Value> {
    payload["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("Items must be a list, got {payload}"))
        .clone()
}

/// One listed job by `JobId`.
fn listed(payload: &Value, job_id: &str) -> Value {
    items(payload)
        .into_iter()
        .find(|job| job["JobId"].as_str() == Some(job_id))
        .unwrap_or_else(|| panic!("the listing must carry {job_id}, got {payload}"))
}

/// List every transformation job, with no filter and no paging.
async fn list_all(service: &HealthLake) -> Value {
    service.ok("ListDataTransformationJobs", &json!({})).await
}

/// The epoch seconds a reported timestamp carries: a timestamp is carried as a
/// number on this protocol, so it is read rather than compared to a literal.
fn epoch_secs(job: &Value, member: &str) -> Option<f64> {
    job.get(member).and_then(Value::as_f64)
}

/// The epoch seconds `BASE_SECS + offset` is, as a reported timestamp carries
/// it.
#[allow(clippy::cast_precision_loss)]
fn reported_secs(offset: i64) -> f64 {
    (BASE_SECS + offset) as f64
}

#[tokio::test]
async fn every_held_job_is_reported_in_submit_order_with_every_summary_member() {
    let service = HealthLake::new();

    // Seeded out of submit order, so a listing in insertion order would fail.
    // The rows differ along which optional summary members they hold and along
    // the `SourceFormat` value, which is drawn from the modelled catalog.
    let expected = seed_jobs(
        &service,
        &[
            Seed::new("job-charlie", Some("nightly"), TransformationJobStatus::Completed)
                .submitted(30)
                .ended(31)
                .source_format("CSV"),
            Seed::new("job-alpha", Some("hourly"), TransformationJobStatus::Submitted)
                .submitted(10)
                .source_format("CCDA"),
            Seed::new("job-bravo", None, TransformationJobStatus::InProgress).submitted(20),
        ],
    );

    let payload = list_all(&service).await;
    assert_eq!(
        job_ids(&payload),
        expected,
        "the listing must report every held job in submit order, got {payload}"
    );
    assert!(
        payload.get("NextToken").is_none(),
        "a single complete page must carry no NextToken, got {payload}"
    );

    // A job holding every optional summary member reports each of them.
    let charlie = listed(&payload, "job-charlie");
    for (member, expected_value) in [
        ("JobId", json!("job-charlie")),
        ("JobName", json!("nightly")),
        ("JobStatus", json!("COMPLETED")),
        ("SourceFormat", json!("CSV")),
    ] {
        assert_eq!(
            charlie[member], expected_value,
            "case {member:?}: the listed job must report {expected_value}, got {charlie}"
        );
    }
    assert_eq!(
        epoch_secs(&charlie, "SubmitTime"),
        Some(reported_secs(30)),
        "the listed job must report the instant it was submitted, got {charlie}"
    );
    assert_eq!(
        epoch_secs(&charlie, "EndTime"),
        Some(reported_secs(31)),
        "the listed job must report the instant it ended, got {charlie}"
    );
    // Nothing the summary shape does not declare is reported — the properties
    // shape's own members belong to `DescribeDataTransformationJob`.
    let reported: Vec<String> = charlie
        .as_object()
        .unwrap_or_else(|| panic!("a listed job must be an object, got {charlie}"))
        .keys()
        .cloned()
        .collect();
    for member in &reported {
        assert!(
            SUMMARY_MEMBERS.contains(&member.as_str()),
            "the listing must report only the modelled summary members, got {member:?} in {charlie}"
        );
    }

    // A job holding none of the optional members reports none of them.
    let bravo = listed(&payload, "job-bravo");
    for absent in ["JobName", "EndTime", "SourceFormat"] {
        assert!(
            bravo.get(absent).is_none(),
            "case {absent:?}: a job holding no {absent} must report none, got {bravo}"
        );
    }
    let alpha = listed(&payload, "job-alpha");
    assert_eq!(
        alpha["SourceFormat"],
        json!("CCDA"),
        "the listed job must report the source format its input names, got {alpha}"
    );
    assert!(
        alpha.get("EndTime").is_none(),
        "a job that has not ended must report no EndTime, got {alpha}"
    );
}

#[tokio::test]
async fn an_account_holding_no_job_is_answered_with_an_empty_page() {
    let service = HealthLake::new();
    let payload = list_all(&service).await;
    assert_eq!(
        items(&payload),
        Vec::<Value>::new(),
        "an account holding no job must be answered with an empty Items list, got {payload}"
    );
    assert!(
        payload.get("NextToken").is_none(),
        "an empty page must carry no NextToken, got {payload}"
    );
}

#[tokio::test]
async fn every_filter_member_narrows_the_listing_to_the_jobs_it_names() {
    let service = HealthLake::new();
    seed_jobs(
        &service,
        &[
            Seed::new("job-alpha", Some("hourly"), TransformationJobStatus::Submitted)
                .submitted(10),
            Seed::new("job-bravo", Some("nightly"), TransformationJobStatus::InProgress)
                .submitted(20),
            Seed::new("job-charlie", Some("nightly"), TransformationJobStatus::Completed)
                .submitted(30),
            Seed::new("job-delta", None, TransformationJobStatus::CompletedWithErrors)
                .submitted(40),
            // A unicode name, which the modelled `DataTransformationJobName`
            // pattern (`^[\p{L}\p{Z}\p{N}_.:/=+\-%@]*$`) admits.
            Seed::new("job-echo", Some("клиника 42"), TransformationJobStatus::Queued)
                .submitted(50),
        ],
    );

    // The rows differ along which filter members are set, the form of the value
    // (a name held by one job, by two, by none, a unicode one), the status
    // (drawn from the modelled `TransformationJobStatus` catalog), and where a
    // cutoff falls relative to a submit instant: on it, one second inside it,
    // one second outside it.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter member set",
            json!({}),
            vec!["job-alpha", "job-bravo", "job-charlie", "job-delta", "job-echo"],
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
            "a unicode JobName",
            json!({ "JobName": "клиника 42" }),
            vec!["job-echo"],
        ),
        (
            "JobName held by no job",
            json!({ "JobName": "weekly" }),
            vec![],
        ),
        (
            "JobStatus held by one job",
            json!({ "JobStatus": "IN_PROGRESS" }),
            vec!["job-bravo"],
        ),
        (
            "JobStatus held by no job",
            json!({ "JobStatus": "FAILED" }),
            vec![],
        ),
        (
            "SubmittedAfter exactly on a submit instant is inclusive",
            json!({ "SubmittedAfter": BASE_SECS + 30 }),
            vec!["job-charlie", "job-delta", "job-echo"],
        ),
        (
            "SubmittedAfter one second past a submit instant excludes it",
            json!({ "SubmittedAfter": BASE_SECS + 31 }),
            vec!["job-delta", "job-echo"],
        ),
        (
            "SubmittedBefore exactly on a submit instant is inclusive",
            json!({ "SubmittedBefore": BASE_SECS + 20 }),
            vec!["job-alpha", "job-bravo"],
        ),
        (
            "SubmittedBefore one second before a submit instant excludes it",
            json!({ "SubmittedBefore": BASE_SECS + 19 }),
            vec!["job-alpha"],
        ),
        (
            "a window spanning the middle two jobs",
            json!({ "SubmittedAfter": BASE_SECS + 20, "SubmittedBefore": BASE_SECS + 30 }),
            vec!["job-bravo", "job-charlie"],
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
                "SubmittedAfter": BASE_SECS + 20,
                "SubmittedBefore": BASE_SECS + 20,
            }),
            vec!["job-bravo"],
        ),
    ];

    for (case, filter, expected) in &rows {
        let payload = service.ok("ListDataTransformationJobs", filter).await;
        let listed_ids = job_ids(&payload);
        assert_eq!(
            listed_ids, *expected,
            "case {case:?}: the listing must report {expected:?}, got {listed_ids:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_transformation_job_status_is_selectable() {
    // The catalog is the generated `TRANSFORMATION_JOB_STATUS_VALUES`, which is
    // the modelled `TransformationJobStatus` enum's own value set: the stored
    // statuses must spell exactly it, so a filter can name every value the
    // model admits.
    let wire: Vec<&str> = EVERY_STATUS.iter().map(|status| status.as_wire()).collect();
    assert_eq!(
        wire,
        TRANSFORMATION_JOB_STATUS_VALUES.to_vec(),
        "the stored statuses must spell the modelled value set, got {wire:?}"
    );

    let service = HealthLake::new();
    // The job's identifier is the status it holds, spelled with the `-` the
    // modelled `DataTransformationJobId` pattern (`^[a-zA-Z0-9-]+$`) admits in
    // place of the `_` it does not, so a row names itself in a failure message.
    let ids: Vec<&'static str> = vec![
        "job-completed",
        "job-completed-with-errors",
        "job-failed",
        "job-in-progress",
        "job-queued",
        "job-submitted",
    ];
    let seeds: Vec<Seed> = EVERY_STATUS
        .iter()
        .zip(&ids)
        .enumerate()
        .map(|(index, (status, id))| {
            Seed::new(id, None, *status).submitted(i64::try_from(index).unwrap_or_default())
        })
        .collect();
    let expected = seed_jobs(&service, &seeds);
    assert_eq!(
        job_ids(&list_all(&service).await),
        expected,
        "an unfiltered listing must report one job per modelled status"
    );

    for (status, id) in EVERY_STATUS.iter().zip(&ids) {
        let case = status.as_wire();
        let payload = service
            .ok("ListDataTransformationJobs", &json!({ "JobStatus": case }))
            .await;
        let listed_ids = job_ids(&payload);
        assert_eq!(
            listed_ids,
            vec![(*id).to_string()],
            "case {case:?}: filtering on a status must report exactly the job holding it, got {listed_ids:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_source_format_is_reported_as_the_input_names_it() {
    // The catalog is the generated `SOURCE_FORMAT_VALUES`, the modelled
    // `SourceFormat` enum's own value set, so a value the model gains cannot
    // stay untested here.
    assert_eq!(
        SOURCE_FORMAT_VALUES.to_vec(),
        vec!["CCDA", "CSV"],
        "the modelled SourceFormat value set must be the one these rows span"
    );

    let service = HealthLake::new();
    let ids = ["job-format-0", "job-format-1"];
    let seeds: Vec<Seed> = SOURCE_FORMAT_VALUES
        .iter()
        .zip(ids)
        .enumerate()
        .map(|(index, (format, id))| {
            Seed::new(id, None, TransformationJobStatus::Completed)
                .submitted(i64::try_from(index).unwrap_or_default())
                .source_format(format)
        })
        .collect();
    seed_jobs(&service, &seeds);

    let payload = list_all(&service).await;
    for (format, id) in SOURCE_FORMAT_VALUES.iter().zip(ids) {
        let job = listed(&payload, id);
        assert_eq!(
            job["SourceFormat"],
            json!(format),
            "case {format:?}: the listed job must report its input's source format, got {job}"
        );
    }
}

/// The identifiers the paging walk seeds, one per entry. Written out rather
/// than formatted so each is a `JobId` the modelled pattern admits.
const PAGED_IDS: [&str; 7] = ["job-0", "job-1", "job-2", "job-3", "job-4", "job-5", "job-6"];

#[tokio::test]
async fn a_page_resumes_exactly_where_its_token_left_off() {
    let service = HealthLake::new();
    let seeds: Vec<Seed> = PAGED_IDS
        .iter()
        .enumerate()
        .map(|(index, id)| {
            Seed::new(id, None, TransformationJobStatus::Completed)
                .submitted(i64::try_from(index).unwrap_or_default())
        })
        .collect();
    let expected = seed_jobs(&service, &seeds);

    // The page sizes span the published `MaxResults` range (1..100): its lower
    // bound, sizes that do and do not divide the seeded count, the exact count,
    // one past it and the upper bound.
    for page_size in [1usize, 2, 3, 6, 7, 8, 100] {
        let case = format!("MaxResults={page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(resume) = &token {
                request["NextToken"] = json!(resume);
            }
            let payload = service.ok("ListDataTransformationJobs", &request).await;
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

    // A request naming no `MaxResults` gets the whole listing, which the model
    // documents as up to 100 results.
    assert_eq!(
        job_ids(&list_all(&service).await),
        expected,
        "a request naming no MaxResults must page at the modelled maximum"
    );

    // A page size outside the modelled 1..100 range is refused by the boundary
    // validator rather than clamped.
    for page_size in [0, 101] {
        let case = format!("MaxResults={page_size}");
        let (status, payload) = service
            .call(
                "ListDataTransformationJobs",
                &json!({ "MaxResults": page_size }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside 1..100 must be refused, got {status}: {payload}"
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
    seed_jobs(
        &service,
        &[Seed::new(
            "job-alpha",
            None,
            TransformationJobStatus::Completed,
        )],
    );

    for (case, token, filter) in [
        ("a token naming no job at all", "job-nothing", json!({})),
        (
            "the identifier of a job the filter excludes",
            "job-alpha",
            json!({ "JobStatus": "FAILED" }),
        ),
    ] {
        let mut request = filter;
        request["NextToken"] = json!(token);
        let (status, payload) = service
            .call("ListDataTransformationJobs", &request)
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
async fn a_status_transition_the_clock_owes_is_reported_once_it_falls_due() {
    let service = HealthLake::new();
    seed_jobs(
        &service,
        &[
            Seed::new("job-moving", None, TransformationJobStatus::Submitted)
                .submitted(10)
                .pending(TransformationJobStatus::InProgress),
            Seed::new("job-ending", None, TransformationJobStatus::InProgress)
                .submitted(20)
                .pending(TransformationJobStatus::Completed),
            Seed::new("job-settled", None, TransformationJobStatus::Queued).submitted(30),
        ],
    );

    // Before the span is up every job still reports the status it was seeded
    // with, so a caller listing straight after a job starts sees the transient
    // one rather than the settled one.
    let payload = list_all(&service).await;
    for (case, expected) in [
        ("job-moving", "SUBMITTED"),
        ("job-ending", "IN_PROGRESS"),
        ("job-settled", "QUEUED"),
    ] {
        let job = listed(&payload, case);
        assert_eq!(
            job["JobStatus"],
            json!(expected),
            "case {case:?}: before the transition falls due the listing must report {expected:?}, got {job}"
        );
        assert!(
            job.get("EndTime").is_none(),
            "case {case:?}: a job that has not ended must report no EndTime, got {job}"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    let payload = list_all(&service).await;
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
    let again = list_all(&service).await;
    assert_eq!(
        listed(&again, "job-ending")["EndTime"],
        listed(&payload, "job-ending")["EndTime"],
        "two listings of one ended job must report the same EndTime"
    );
}

#[tokio::test]
async fn a_started_job_is_listed_exactly_as_describe_reports_it() {
    let service = HealthLake::new();
    let (status, created) = service
        .call(
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "ccda-to-r4",
                "SourceFormat": "CCDA",
                "Source": { "ProfileMapping": { "ProfileMapping": {
                    "sections/allergies.map": "AllergyIntolerance <- allergy",
                } } },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "creating the profile must answer the protocol's 200, got {status}: {created}"
    );
    let profile_id = string_at(&created, "ProfileId");

    // The rows differ along the members a start carries that a summary
    // reports: the `JobName` and the input's `SourceFormat`.
    let rows = [
        ("named, CCDA source", "token-ccda", Some("nightly"), Some("CCDA")),
        ("unnamed, CSV source", "token-csv", None, Some("CSV")),
        ("named, no source format", "token-plain", Some("hourly"), None),
    ];
    let mut started_ids: Vec<String> = Vec::new();
    for (case, token, name, source_format) in rows {
        let mut input = json!({ "S3Uri": "s3://transform-in-bucket/incoming" });
        if let Some(format) = source_format {
            input["SourceFormat"] = json!(format);
        }
        let mut request = json!({
            "ClientToken": token,
            "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
            "InputDataConfig": input,
            "OutputDataConfig": { "S3Configuration": {
                "S3Uri": "s3://transform-out-bucket/r4",
                "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            } },
            "ProfileId": profile_id,
        });
        if let Some(job_name) = name {
            request["JobName"] = json!(job_name);
        }
        let begun = service.ok("StartDataTransformationJob", &request).await;
        let job_id = string_at(&begun, "JobId");
        started_ids.push(job_id.clone());

        // The listing reports the job the start minted, out of the SAME record
        // `DescribeDataTransformationJob` reads.
        let payload = list_all(&service).await;
        let job = listed(&payload, &job_id);
        let described = service
            .ok("DescribeDataTransformationJob", &json!({ "JobId": job_id }))
            .await["TransformationJobProperties"]
            .clone();
        for member in ["JobId", "JobStatus", "SubmitTime", "JobName", "EndTime"] {
            assert_eq!(
                job.get(member),
                described.get(member),
                "case {case:?}: the listing must report {member} as the description does, got {job} and {described}"
            );
        }
        assert_eq!(
            job.get("SourceFormat").and_then(Value::as_str),
            source_format,
            "case {case:?}: the listing must report the source format the start named, got {job}"
        );
    }

    // Every started job is listed, and only those.
    let listed_ids = job_ids(&list_all(&service).await);
    assert_eq!(
        listed_ids.len(),
        started_ids.len(),
        "the listing must report exactly the started jobs, got {listed_ids:?}"
    );
    for id in &started_ids {
        assert!(
            listed_ids.contains(id),
            "the listing must report the started job {id:?}, got {listed_ids:?}"
        );
    }

    // Once the clock has settled the started jobs the listing reports the
    // terminal status and the instant each ended, the same record the
    // description reports.
    tokio::time::sleep(PAST_TRANSITION).await;
    let payload = list_all(&service).await;
    for id in &started_ids {
        let job = listed(&payload, id);
        assert_eq!(
            job["JobStatus"],
            json!("FAILED"),
            "once due the listing must report the settled status, got {job}"
        );
        assert!(
            job["EndTime"].is_number(),
            "a job that reached a terminal status must report an EndTime, got {job}"
        );
    }
}
