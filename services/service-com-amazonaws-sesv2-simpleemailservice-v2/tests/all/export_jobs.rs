// SPDX-License-Identifier: CC0-1.0

//! The export-job listing (`ListExportJobs`), end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reaches the engine's own state to seed the stored jobs a listing
//! reports and to move the instants the clock-owed transitions of a run fall due at.
//!
//! The rules these cases pin:
//!
//! * the listing is computed from the stored job records: every stored job is reported
//!   once, as the summary its record projects (its id, its source type, the status of
//!   its run and when it was created and completed), most recently created first;
//! * `ExportSourceType` and `JobStatus` narrow the listing to the jobs that carry the
//!   value the request names, and a modelled value no stored job carries matches
//!   nothing;
//! * `PageSize` truncates a page and `NextToken` resumes the walk strictly after the
//!   position the previous page ended at, so a walk reports every matching job exactly
//!   once and terminates;
//! * a run's `CREATED` -> `PROCESSING` -> `COMPLETED` transitions are owed by the
//!   clock, not by being read: a listing materialises whichever of them were already
//!   due, and never moves a run on because somebody looked;
//! * a token this listing never handed out is refused with the one input error the
//!   operation models, and a filter value outside its modelled enum is refused at the
//!   request boundary.
//!
//! The tables below span what the rules generalise over: the form and length of a job
//! id, both modelled source types, every modelled job status, the number of stored
//! jobs, the page size, and every refusal leg.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    EXPORT_SOURCE_TYPE_VALUES, JOB_STATUS_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    encode_tagged_page_token, ComAmazonawsSesv2SimpleemailserviceV2State, ExportJobRecord,
    EXPORT_JOB_COMPLETION_SETTLE, EXPORT_JOB_PROCESSING_SETTLE, TENANTS_PAGE_TOKEN_TAG,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An export job belongs to the account, so the listing is the same from any
/// Region; one is enough.
const HOME: &str = "us-east-1";

/// Long enough that a transition held this far out cannot fall due while a case runs.
const HELD: Duration = Duration::from_secs(3600);

/// Just enough to put a due instant in the past.
const ELAPSED: Duration = Duration::from_secs(1);

/// The instant the first seeded job was created at, as whole epoch seconds so the
/// timestamp a response carries compares exactly.
const FIRST_CREATED_AT_SECS: u64 = 1_761_580_651;

/// How far apart the seeded jobs were created, so creation order is unambiguous.
const CREATED_APART: Duration = Duration::from_secs(60);

/// `ExportSourceType$METRICS_DATA` — "the metrics export".
const METRICS_DATA: &str = "METRICS_DATA";

/// `ExportSourceType$MESSAGE_INSIGHTS` — "the Message Insights export".
const MESSAGE_INSIGHTS: &str = "MESSAGE_INSIGHTS";

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The summaries the listing reported, in the order it reported them.
    fn jobs(&self) -> Vec<Value> {
        self.body
            .get("ExportJobs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The ids the listing reported, in the order it reported them.
    fn ids(&self) -> Vec<String> {
        self.jobs()
            .iter()
            .map(|job| {
                job.get("JobId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The token the listing handed back, if any.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A member of one reported summary, as the document carries it: `None` when the
/// member is absent altogether.
fn member(job: &Value, name: &str) -> Option<Value> {
    job.get(name).cloned()
}

/// How far a seeded job's run has got.
///
/// The handler stores a due instant per transition rather than sleeping, so a case
/// drives a run by moving those instants instead of waiting on the wall clock — which
/// keeps every assertion independent of how loaded the machine is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    /// The run has not started: both transitions are still ahead.
    Created,
    /// The run started and has not finished.
    Processing,
    /// The run finished.
    Completed,
}

impl Run {
    /// The modelled `JobStatus` a job in this state reports.
    fn status(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
        }
    }
}

/// One seeded export job.
struct JobRow {
    case: &'static str,
    job_id: &'static str,
    source_type: &'static str,
    run: Run,
}

/// The population the listing cases seed, oldest first.
///
/// The rows span what the rules generalise over: both values of `ExportSourceType`
/// (`smithy_query(query="shape",shape="ExportSourceType")`), every status a run of this
/// emulator reaches, and `JobId` forms of different shapes and lengths — the model
/// bounds a job id only at one character or more.
const JOB_ROWS: &[JobRow] = &[
    JobRow {
        case: "a dashed id, metrics data, run not started",
        job_id: "7d1a0f8c-0f4a-4f1e-9d2f-2a1b3c4d5e6f",
        source_type: METRICS_DATA,
        run: Run::Created,
    },
    JobRow {
        case: "a one-character id, message insights, run not started",
        job_id: "a",
        source_type: MESSAGE_INSIGHTS,
        run: Run::Created,
    },
    JobRow {
        case: "a hexadecimal id, metrics data, run in progress",
        job_id: "0f3b2a1c9d8e7f60",
        source_type: METRICS_DATA,
        run: Run::Processing,
    },
    JobRow {
        case: "an id of digits, message insights, run in progress",
        job_id: "20261027161731",
        source_type: MESSAGE_INSIGHTS,
        run: Run::Processing,
    },
    JobRow {
        case: "a mixed-case id with separators, metrics data, run finished",
        job_id: "Export_Job-42",
        source_type: METRICS_DATA,
        run: Run::Completed,
    },
    JobRow {
        case: "a long id, message insights, run finished",
        job_id: "insights-0123456789-0123456789-0123456789-0123456789-0123456789",
        source_type: MESSAGE_INSIGHTS,
        run: Run::Completed,
    },
];

/// The emulated service behind its router, plus the shared engine so a case can seed
/// the stored jobs and move the instants their transitions fall due at.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Send one `ListExportJobs` request with `body` as its document.
    async fn list(&self, body: Value) -> Reply {
        let payload = bytes::Bytes::from(serde_json::to_vec(&body).expect("serialize body"));
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/v2/email/list-export-jobs"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// The whole listing, with no filter and no page size.
    async fn list_all(&self) -> Reply {
        let reply = self.list(json!({})).await;
        assert_eq!(
            reply.status, 200,
            "the listing must succeed, got {} {:?}",
            reply.status, reply.body
        );
        reply
    }

    /// Store one export job as a create would, then put its run where `run` says by
    /// moving the instants its transitions fall due at.
    fn seed(&self, job_id: &str, source_type: &str, created_at: SystemTime, run: Run) {
        let now = SystemTime::now();
        let (processing_due_at, completion_due_at) = match run {
            Run::Created => (now + HELD, now + HELD + HELD),
            Run::Processing => (now - ELAPSED, now + HELD),
            Run::Completed => (now - ELAPSED - ELAPSED, now - ELAPSED),
        };
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let mut record = ExportJobRecord::new(job_id.to_string(), source_type, created_at);
        record.processing_due_at = processing_due_at;
        record.completion_due_at = completion_due_at;
        store.export_jobs.insert(job_id.to_string(), record);
    }

    /// Store one export job exactly as a create leaves it: just created, with both
    /// transitions of its run due at the spans the state module states.
    fn seed_fresh(&self, job_id: &str, source_type: &str, created_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.export_jobs.insert(
            job_id.to_string(),
            ExportJobRecord::new(job_id.to_string(), source_type, created_at),
        );
    }

    /// Seed [`JOB_ROWS`] oldest first, each created [`CREATED_APART`] after the one
    /// before it.
    fn seed_population(&self) {
        for (index, row) in JOB_ROWS.iter().enumerate() {
            self.seed(row.job_id, row.source_type, created_at(index), row.run);
        }
    }

    /// Move the instants `job_id`'s transitions fall due at.
    fn set_due(&self, job_id: &str, processing_due_at: SystemTime, completion_due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .export_jobs
            .get_mut(job_id)
            .unwrap_or_else(|| panic!("case {job_id:?}: the job must be in the store"));
        record.processing_due_at = processing_due_at;
        record.completion_due_at = completion_due_at;
    }

    /// Hold both of `job_id`'s transitions off, so an assertion about the status it
    /// was created in cannot race the span.
    fn hold_created(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now + HELD, now + HELD + HELD);
    }

    /// Bring the start of `job_id`'s run due, without waiting for it, and hold its
    /// completion off.
    fn bring_processing_due(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now - ELAPSED, now + HELD);
    }

    /// Bring the completion of `job_id`'s run due, without waiting for it, at a whole
    /// epoch second so the timestamp it is reported with compares exactly.
    fn bring_completion_due(&self, job_id: &str) -> SystemTime {
        let now = SystemTime::now();
        let completed_at = SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                now.duration_since(SystemTime::UNIX_EPOCH)
                    .expect("the clock is after the epoch")
                    .as_secs()
                    - ELAPSED.as_secs(),
            );
        self.set_due(job_id, now - ELAPSED - ELAPSED, completed_at);
        completed_at
    }

    /// The spans the record stores between `job_id`'s creation and each of its
    /// transitions falling due.
    fn stored_spans(&self, job_id: &str) -> (Duration, Duration) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let record = store
            .export_jobs
            .get(job_id)
            .unwrap_or_else(|| panic!("case {job_id:?}: the job must be in the store"));
        (
            record
                .processing_due_at
                .duration_since(record.created_at)
                .unwrap_or_default(),
            record
                .completion_due_at
                .duration_since(record.created_at)
                .unwrap_or_default(),
        )
    }

    /// The instant the store holds as when `job_id`'s run completed, if it has.
    fn stored_completed_at(&self, job_id: &str) -> Option<SystemTime> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .export_jobs
            .get(job_id)
            .and_then(|record| record.completed_at)
    }

    /// Forget one stored job, so a case can show that a token names a position in the
    /// order rather than the job it was minted from.
    fn forget(&self, job_id: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.export_jobs.remove(job_id);
    }
}

/// When the job seeded at `index` of [`JOB_ROWS`] was created.
fn created_at(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(FIRST_CREATED_AT_SECS) + CREATED_APART * (index as u32)
}

/// The ids of [`JOB_ROWS`] in the order a listing reports them: most recently created
/// first.
fn newest_first() -> Vec<String> {
    JOB_ROWS
        .iter()
        .rev()
        .map(|row| row.job_id.to_string())
        .collect()
}

#[tokio::test]
async fn every_stored_job_is_reported_as_its_record_projects_it() {
    let ses = Ses::new();
    ses.seed_population();

    let reply = ses.list_all().await;
    assert_eq!(
        reply.ids(),
        newest_first(),
        "expected every stored job once, most recently created first, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.next_token(),
        None,
        "the whole listing fits in one page, so there is nothing to resume from, got {:?}",
        reply.body
    );

    let reported = reply.jobs();
    for (index, row) in JOB_ROWS.iter().enumerate().rev() {
        let case = row.case;
        // The listing is newest first, so the oldest row is reported last.
        let job = &reported[JOB_ROWS.len() - 1 - index];
        assert_eq!(
            member(job, "JobId"),
            Some(json!(row.job_id)),
            "case {case:?}: expected the stored job id, got {job:?}"
        );
        assert_eq!(
            member(job, "ExportSourceType"),
            Some(json!(row.source_type)),
            "case {case:?}: expected the stored source type, got {job:?}"
        );
        assert_eq!(
            member(job, "JobStatus"),
            Some(json!(row.run.status())),
            "case {case:?}: expected the status the run has reached, got {job:?}"
        );
        let expected_created = created_at(index)
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("seeded after the epoch")
            .as_secs();
        assert_eq!(
            member(job, "CreatedTimestamp"),
            Some(json!(expected_created)),
            "case {case:?}: expected the instant the job was created, got {job:?}"
        );
        // "The timestamp of when the export job was completed": present only for a run
        // that has one.
        let completed = member(job, "CompletedTimestamp");
        if row.run == Run::Completed {
            let stored = ses
                .stored_completed_at(row.job_id)
                .unwrap_or_else(|| panic!("case {case:?}: a finished run has a completion instant"));
            let reported_secs = completed
                .as_ref()
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected a CompletedTimestamp, got {job:?}"));
            let stored_secs = stored
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("completed after the epoch")
                .as_secs_f64();
            assert!(
                (reported_secs - stored_secs).abs() < 0.002,
                "case {case:?}: expected the stored completion instant {stored_secs}, got {reported_secs}"
            );
        } else {
            assert_eq!(
                completed, None,
                "case {case:?}: a run that has not finished has no completion instant, got {job:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_filters_narrow_the_listing_to_the_jobs_that_carry_the_value() {
    // The filter value sets are the modelled enums', so every one of them is a case —
    // including the two statuses no run of this emulator reaches, which must match
    // nothing rather than everything.
    assert_eq!(
        EXPORT_SOURCE_TYPE_VALUES,
        &["MESSAGE_INSIGHTS", "METRICS_DATA"],
        "the source types this case spans must be the modelled set, got {EXPORT_SOURCE_TYPE_VALUES:?}"
    );
    assert_eq!(
        JOB_STATUS_VALUES,
        &["CANCELLED", "COMPLETED", "CREATED", "FAILED", "PROCESSING"],
        "the statuses this case spans must be the modelled set, got {JOB_STATUS_VALUES:?}"
    );

    let ses = Ses::new();
    ses.seed_population();

    let source_filters: Vec<Option<&str>> = std::iter::once(None)
        .chain(EXPORT_SOURCE_TYPE_VALUES.iter().copied().map(Some))
        .collect();
    let status_filters: Vec<Option<&str>> = std::iter::once(None)
        .chain(JOB_STATUS_VALUES.iter().copied().map(Some))
        .collect();

    for source in &source_filters {
        for status in &status_filters {
            let case = format!("ExportSourceType={source:?}, JobStatus={status:?}");
            let mut body = json!({});
            if let Some(source) = source {
                body["ExportSourceType"] = json!(source);
            }
            if let Some(status) = status {
                body["JobStatus"] = json!(status);
            }
            let reply = ses.list(body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a modelled filter value is served, got {} {:?}",
                reply.status, reply.body
            );
            let expected: Vec<String> = JOB_ROWS
                .iter()
                .rev()
                .filter(|row| source.is_none_or(|wanted| row.source_type == wanted))
                .filter(|row| status.is_none_or(|wanted| row.run.status() == wanted))
                .map(|row| row.job_id.to_string())
                .collect();
            assert_eq!(
                reply.ids(),
                expected,
                "case {case:?}: expected exactly the jobs carrying the value, got {:?}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_page_walk_reports_every_matching_job_exactly_once() {
    let ses = Ses::new();
    ses.seed_population();
    let all = newest_first();

    // A page size below, at and above the number of stored jobs, plus the two forms
    // that truncate nothing: an absent size and a size of zero.
    let sizes = [Some(1), Some(2), Some(3), Some(JOB_ROWS.len()), Some(JOB_ROWS.len() + 2), Some(0), None];
    for size in sizes {
        let case = format!("PageSize={size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One request per job is the most a terminating walk can need; the guard turns
        // a walk that fails to terminate into a failed assertion instead of a hang.
        for _ in 0..=JOB_ROWS.len() {
            let mut body = json!({});
            if let Some(size) = size {
                body["PageSize"] = json!(size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let reply = ses.list(body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the page must be served, got {} {:?}",
                reply.status, reply.body
            );
            let page = reply.ids();
            if let Some(size) = size.filter(|size| *size > 0) {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page may not exceed the size asked for, got {page:?}"
                );
            }
            walked.extend(page);
            token = reply.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the walk must terminate, and it still offered {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: expected the whole listing, in order, exactly once"
        );
    }
}

#[tokio::test]
async fn a_token_names_a_position_not_the_job_it_was_minted_from() {
    let ses = Ses::new();
    ses.seed_population();
    let all = newest_first();

    let first = ses.list(json!({"PageSize": 1})).await;
    assert_eq!(
        first.ids(),
        all[..1].to_vec(),
        "expected the newest job on the first page, got {:?}",
        first.body
    );
    let token = first
        .next_token()
        .expect("jobs remain behind the first page, so it carries a token");

    // The job the token was minted from is gone: the token is still a position in the
    // order, so the walk resumes exactly where it was.
    ses.forget(&all[0]);
    let second = ses.list(json!({"NextToken": token})).await;
    assert_eq!(
        second.status, 200,
        "a position whose job is gone is still a position, got {} {:?}",
        second.status, second.body
    );
    assert_eq!(
        second.ids(),
        all[1..].to_vec(),
        "expected the rest of the listing, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn a_runs_transitions_fall_due_on_the_clock_and_never_on_a_read() {
    let ses = Ses::new();
    let job_id = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";
    ses.seed_fresh(job_id, METRICS_DATA, SystemTime::now());

    let (processing_span, completion_span) = ses.stored_spans(job_id);
    assert_eq!(
        processing_span, EXPORT_JOB_PROCESSING_SETTLE,
        "expected the run to start {EXPORT_JOB_PROCESSING_SETTLE:?} after the job was created, \
         got {processing_span:?}"
    );
    assert_eq!(
        completion_span,
        EXPORT_JOB_PROCESSING_SETTLE + EXPORT_JOB_COMPLETION_SETTLE,
        "expected the run to finish {:?} after the job was created, got {completion_span:?}",
        EXPORT_JOB_PROCESSING_SETTLE + EXPORT_JOB_COMPLETION_SETTLE
    );

    // Nothing is due yet, and reading the job twice does not move its run on: the
    // transition is owed by the clock, not by the read.
    ses.hold_created(job_id);
    for read in ["first", "second"] {
        let reply = ses.list_all().await;
        let job = reply
            .jobs()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {read:?}: the stored job must be reported"));
        assert_eq!(
            member(&job, "JobStatus"),
            Some(json!(Run::Created.status())),
            "case {read:?}: nothing is due yet, so the run has not started, got {job:?}"
        );
        assert_eq!(
            member(&job, "CompletedTimestamp"),
            None,
            "case {read:?}: a run that has not finished has no completion instant, got {job:?}"
        );
    }

    ses.bring_processing_due(job_id);
    let processing = ses.list_all().await;
    let job = processing
        .jobs()
        .first()
        .cloned()
        .expect("the stored job must be reported");
    assert_eq!(
        member(&job, "JobStatus"),
        Some(json!(Run::Processing.status())),
        "the start of the run was due, so the listing reports it processing, got {job:?}"
    );
    assert_eq!(
        member(&job, "CompletedTimestamp"),
        None,
        "the run has not finished, so it has no completion instant, got {job:?}"
    );

    let completed_at = ses.bring_completion_due(job_id);
    let completed = ses.list_all().await;
    let job = completed
        .jobs()
        .first()
        .cloned()
        .expect("the stored job must be reported");
    assert_eq!(
        member(&job, "JobStatus"),
        Some(json!(Run::Completed.status())),
        "the completion of the run was due, so the listing reports it completed, got {job:?}"
    );
    let expected_secs = completed_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("completed after the epoch")
        .as_secs();
    assert_eq!(
        member(&job, "CompletedTimestamp"),
        Some(json!(expected_secs)),
        "expected the instant the run was due to finish, got {job:?}"
    );
    assert_eq!(
        ses.stored_completed_at(job_id),
        Some(completed_at),
        "the stored completion instant is the one that fell due, not the one it was read at"
    );
}

#[tokio::test]
async fn a_token_this_listing_never_handed_out_is_refused() {
    let ses = Ses::new();
    ses.seed_population();

    let cases: Vec<(&str, String)> = vec![
        ("a token that is not base64 at all", "not-a-token".to_string()),
        (
            "base64 of something this service never minted",
            aws_smithy_types::base64::encode("just some text"),
        ),
        (
            "another listing's token",
            encode_tagged_page_token(TENANTS_PAGE_TOKEN_TAG, "tenant-a"),
        ),
        ("an empty token", String::new()),
    ];

    for (case, token) in cases {
        let reply = ses.list(json!({"NextToken": token})).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            "Invalid next token.",
            "case {case:?}: expected the refusal this service words for a token that is not a \
             position in the listing, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_filter_value_outside_its_modelled_enum_is_refused() {
    let ses = Ses::new();
    ses.seed_population();

    // Each row names the member and a value outside its modelled value set: a value
    // the enum does not carry, and one that differs from a modelled value only in
    // case or in surrounding space.
    let cases = [
        ("ExportSourceType", "exportSourceType", "METRICS"),
        ("ExportSourceType", "exportSourceType", "metrics_data"),
        ("ExportSourceType", "exportSourceType", " METRICS_DATA"),
        ("JobStatus", "jobStatus", "DONE"),
        ("JobStatus", "jobStatus", "created"),
        ("JobStatus", "jobStatus", "COMPLETED "),
    ];

    for (member_name, member_path, value) in cases {
        let case = format!("{member_name}={value:?}");
        let mut body = json!({});
        body[member_name] = json!(value);
        let reply = ses.list(body).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        // The clause names the member and the value set it had to satisfy, and
        // withholds the value that violated them.
        let subject = format!(
            "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
             Member must satisfy enum value set: ["
        );
        let message = reply.message();
        assert!(
            message.starts_with(&subject),
            "case {case:?}: expected the refusal to open {subject:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_account_holding_no_export_job_has_an_empty_listing() {
    let ses = Ses::new();

    let reply = ses.list_all().await;
    assert_eq!(
        reply.jobs(),
        Vec::<Value>::new(),
        "expected an empty list, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.body.get("ExportJobs"),
        Some(&json!([])),
        "an empty list is a different answer from an absent member, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.next_token(),
        None,
        "there is nothing to resume from, got {:?}",
        reply.body
    );
}
