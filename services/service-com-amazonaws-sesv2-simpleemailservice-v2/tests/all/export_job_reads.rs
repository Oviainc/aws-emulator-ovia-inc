// SPDX-License-Identifier: CC0-1.0

//! Reading one export job (`GetExportJob`) and downloading the file its run wrote,
//! end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised; a case reaches the engine's own state only to place a message in the
//! outbound log at the instant it is about and to move the instants a run's
//! clock-owed transitions fall due at.
//!
//! The rules these cases pin:
//!
//! * a read of an export job is computed from the stored record: its id, its source
//!   type, the status of its run, the data source the create supplied, the data format
//!   the create asked for, and when the job was created and completed;
//! * "when the job status is COMPLETED, the response includes a pre-signed URL in
//!   `ExportDestination.S3Url` that you use to download the export file" — so a run
//!   that has not completed reports no URL and no statistics, and one that has reports
//!   both;
//! * the URL leads back to this emulator and serves the file the run wrote: the
//!   records the job's data source selects, rendered in the data format it asks for,
//!   with the statistics counting exactly those records;
//! * a metrics export file's shape derives from the request — a field for the day, one
//!   per dimension the request names and one per metric and aggregation it asks for —
//!   and each metric's value is the one a metric query reports for the same window and
//!   dimensions;
//! * a Message Insights export file holds one record per recipient of each logged
//!   message inside the requested interval, kept or dropped by the request's own
//!   `Include` and `Exclude` filters;
//! * a `JobId` the account holds no export job for is the modelled `NotFoundException`.
//!
//! The tables below span what the rules generalise over: both modelled source types,
//! both modelled data formats, every run state, the dimension values and metric
//! aggregations a metrics export names, and each filter member a Message Insights
//! export selects on.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, ExportJobStatus, SentMessage,
    MAX_EXPORT_FILES_PER_CALL,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An export job belongs to the account, so one Region is enough.
const HOME: &str = "us-east-1";

/// `2026-01-01T00:00:00Z`, the UTC day the logged messages are accepted on, as whole
/// epoch seconds so a timestamp compares exactly.
const DAY_START: i64 = 1_767_225_600;

/// The UTC day [`DAY_START`] begins, as a metrics export record carries it.
const DAY: &str = "2026-01-01";

/// One second short of the next day, so the interval `DAY_START..=DAY_END` covers
/// exactly one UTC day.
const DAY_END: i64 = DAY_START + 86_399;

/// An instant on the day AFTER the interval, for a message the interval must leave
/// out.
const NEXT_DAY: i64 = DAY_START + 86_400 + 7_200;

/// `ExportSourceType$METRICS_DATA` — "the metrics export".
const METRICS_DATA: &str = "METRICS_DATA";

/// `ExportSourceType$MESSAGE_INSIGHTS` — "the Message Insights export".
const MESSAGE_INSIGHTS: &str = "MESSAGE_INSIGHTS";

/// `DataFormat$CSV` — "a comma-separated values file".
const CSV: &str = "CSV";

/// `DataFormat$JSON` — "a Json file".
const JSON: &str = "JSON";

/// `MetricNamespace$VDM` — "the metrics namespace - e.g., VDM".
const VDM: &str = "VDM";

/// Long enough that a transition held this far out cannot fall due while a case runs.
const HELD: Duration = Duration::from_secs(3600);

/// Just enough to put a due instant in the past.
const ELAPSED: Duration = Duration::from_secs(1);

/// One answered API request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the answered document, as it carries it.
    fn member(&self, name: &str) -> Option<Value> {
        self.body.get(name).cloned()
    }

    /// A member of the answered document as text.
    fn text(&self, name: &str) -> String {
        self.member(name)
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// The download URL the destination carries, if any.
    fn s3_url(&self) -> Option<String> {
        self.body
            .get("ExportDestination")
            .and_then(|destination| destination.get("S3Url"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The data format the destination reports.
    fn data_format(&self) -> String {
        self.body
            .get("ExportDestination")
            .and_then(|destination| destination.get("DataFormat"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `(ProcessedRecordsCount, ExportedRecordsCount)`, or `None` when the answer
    /// carries no statistics at all.
    fn statistics(&self) -> Option<(i64, i64)> {
        let statistics = self.body.get("Statistics")?;
        let count = |member: &str| {
            statistics
                .get(member)
                .and_then(Value::as_i64)
                .unwrap_or_default()
        };
        Some((
            count("ProcessedRecordsCount"),
            count("ExportedRecordsCount"),
        ))
    }
}

/// One fetched file: the status, the media type it was served as, and its bytes as
/// text.
struct Fetched {
    status: u16,
    media_type: String,
    body: String,
}

impl Fetched {
    /// The file read as a table: the field names, then one row of cells per record.
    ///
    /// A `CSV` file is split on its own separators; the cells of these cases carry no
    /// quoted separator, which the header assertions show.
    fn csv_table(&self) -> (Vec<String>, Vec<Vec<String>>) {
        let mut lines = self.body.lines();
        let header = lines
            .next()
            .unwrap_or_default()
            .split(',')
            .map(str::to_string)
            .collect();
        let rows = lines
            .map(|line| line.split(',').map(str::to_string).collect())
            .collect();
        (header, rows)
    }

    /// The file read as the JSON array of records it is.
    fn json_records(&self) -> Vec<Value> {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|document| document.as_array().cloned())
            .unwrap_or_default()
    }
}

/// One message a case places in the outbound log.
struct LoggedMessage {
    from: &'static str,
    to: Vec<&'static str>,
    cc: Vec<&'static str>,
    subject: Option<&'static str>,
    /// When it was accepted, in seconds since the epoch.
    at: i64,
}

impl LoggedMessage {
    /// A message from `from` to the single recipient `to`, accepted at `at`.
    fn simple(from: &'static str, to: &'static str, at: i64) -> Self {
        Self {
            from,
            to: vec![to],
            cc: Vec::new(),
            subject: None,
            at,
        }
    }

    /// The same, carrying a subject.
    fn titled(from: &'static str, to: &'static str, subject: &'static str, at: i64) -> Self {
        Self {
            subject: Some(subject),
            ..Self::simple(from, to, at)
        }
    }
}

/// The emulated service behind its router, plus the shared engine so a case can place
/// a message in the log and move the instants a run's transitions fall due at.
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

    /// Send one API request.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"
                ),
            );
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let response = self
            .router
            .dispatch(request.body(payload).expect("build request"))
            .await;
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

    /// Create one export job and answer the `JobId` it minted.
    async fn create(&self, data_source: Value, data_format: &str) -> String {
        let reply = self
            .call(
                "POST",
                "/v2/email/export-jobs",
                Some(json!({
                    "ExportDataSource": data_source,
                    "ExportDestination": {"DataFormat": data_format},
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"setup\": expected 200 from the create, got {} {:?}",
            reply.status, reply.body
        );
        let job_id = reply.text("JobId");
        assert!(
            !job_id.is_empty(),
            "case \"setup\": the create must answer a JobId, got {:?}",
            reply.body
        );
        job_id
    }

    /// Create one email identity, so a dimension that names it selects the messages
    /// it authorised.
    async fn create_identity(&self, identity: &str) {
        let reply = self
            .call(
                "POST",
                "/v2/email/identities",
                Some(json!({"EmailIdentity": identity})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"setup\": expected 200 from the identity create, got {} {:?}",
            reply.status, reply.body
        );
    }

    /// Read one export job.
    async fn get(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/export-jobs/{job_id}"), None)
            .await
    }

    /// Fetch one URL this emulator handed out, exactly as it was handed out.
    async fn fetch(&self, url: &str) -> Fetched {
        let host = url
            .parse::<http::Uri>()
            .ok()
            .and_then(|uri| uri.authority().map(|authority| authority.to_string()))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method("GET")
            .uri(url)
            .header("host", host)
            .body(bytes::Bytes::new())
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let media_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Fetched {
            status,
            media_type,
            body: String::from_utf8_lossy(&response.into_body()).to_string(),
        }
    }

    /// Append a message to the outbound log as if it had been accepted at its own
    /// instant, so a case can place one in a day of its choosing.
    fn log(&self, message: LoggedMessage) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let ordinal = store.sent_messages.len() + 1;
        store.sent_messages.push(SentMessage {
            message_id: format!("logged-{ordinal}"),
            from: Some(message.from.to_string()),
            to: message.to.iter().map(|to| (*to).to_string()).collect(),
            cc: message.cc.iter().map(|cc| (*cc).to_string()).collect(),
            bcc: Vec::new(),
            subject: message.subject.map(str::to_string),
            body: None,
            body_html: None,
            configuration_set_name: None,
            reply_to: Vec::new(),
            feedback_forwarding: None,
            email_tags: Vec::new(),
            headers: Vec::new(),
            sent_at: SystemTime::UNIX_EPOCH + Duration::from_secs(message.at as u64),
        });
    }

    /// Put `job_id`'s run where `run` says by moving the instants its transitions fall
    /// due at. The handler stores a due instant per transition rather than sleeping, so
    /// a case drives a run by moving those instants instead of waiting on the wall
    /// clock.
    fn drive(&self, job_id: &str, run: Run) {
        let now = SystemTime::now();
        let (processing, completion) = match run {
            Run::Created => (now + HELD, now + HELD + HELD),
            Run::Processing => (now - ELAPSED, now + HELD),
            Run::Completed => (now - ELAPSED - ELAPSED, now - ELAPSED),
        };
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .export_jobs
            .get_mut(job_id)
            .expect("the seeded job is stored");
        record.processing_due_at = processing;
        record.completion_due_at = completion;
    }

    /// Take the file off every stored job and put its run back before completion, so a
    /// case can count the files ONE later call writes.
    fn clear_files(&self) {
        let now = SystemTime::now();
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        for record in store.export_jobs.values_mut() {
            record.file = None;
            record.completed_at = None;
            record.status = ExportJobStatus::Processing;
            record.processing_due_at = now - ELAPSED - ELAPSED;
            record.completion_due_at = now - ELAPSED;
        }
    }

    /// How many stored jobs have written their export file.
    fn jobs_with_a_file(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .export_jobs
            .values()
            .filter(|record| record.file.is_some())
            .count()
    }

    /// How many stored jobs report a completed run.
    fn completed_jobs(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .export_jobs
            .values()
            .filter(|record| record.status == ExportJobStatus::Completed)
            .count()
    }
}

/// How far a job's run has got.
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

/// A metrics data source over the one logged day: `dimensions` to split the activity
/// by and `metrics` to report for each.
fn metrics_source(dimensions: Value, metrics: Value) -> Value {
    json!({
        "MetricsDataSource": {
            "Dimensions": dimensions,
            "Namespace": VDM,
            "Metrics": metrics,
            "StartDate": DAY_START,
            "EndDate": DAY_END,
        }
    })
}

/// A Message Insights data source over the one logged day, with whatever filters
/// `filters` adds.
fn insights_source(filters: Value) -> Value {
    let mut source = json!({"StartDate": DAY_START, "EndDate": DAY_END});
    if let Value::Object(members) = filters {
        for (name, value) in members {
            source[name] = value;
        }
    }
    json!({"MessageInsightsDataSource": source})
}

/// One `ExportMetric`: a metric and the aggregation to apply to it.
fn metric(name: &str, aggregation: &str) -> Value {
    json!({"Name": name, "Aggregation": aggregation})
}

/// The path part of an absolute URL, as an API request addresses it.
fn path_of(url: &str) -> &str {
    let after_scheme = url.split_once("//").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .find('/')
        .and_then(|at| after_scheme.get(at..))
        .unwrap_or("/")
}

/// The messages every case's log holds: two to one provider and one to another on the
/// logged day, and one the day after, which no interval of these cases covers. Both
/// sending identities are created, so a dimension naming one of them partitions the
/// activity by the identity that authorised it.
async fn with_logged_day() -> Ses {
    let ses = Ses::new();
    ses.create_identity("sender@example.com").await;
    ses.create_identity("other@example.com").await;
    ses.log(LoggedMessage::titled(
        "sender@example.com",
        "first@gmail.com",
        "Weekly digest",
        DAY_START + 3_600,
    ));
    ses.log(LoggedMessage::titled(
        "sender@example.com",
        "second@gmail.com",
        "Receipt 4821",
        DAY_START + 7_200,
    ));
    ses.log(LoggedMessage::titled(
        "other@example.com",
        "third@yahoo.com",
        "Weekly digest",
        DAY_START + 10_800,
    ));
    ses.log(LoggedMessage::simple(
        "sender@example.com",
        "late@gmail.com",
        NEXT_DAY,
    ));
    ses
}

/// One row of the run-state table.
struct RunRow {
    case: &'static str,
    source_type: &'static str,
    data_format: &'static str,
    run: Run,
}

/// Every pairing of a source type, a data format and a run state: what a read reports
/// is the record's, and the URL and the statistics appear exactly when the run has
/// written a file.
const RUN_ROWS: &[RunRow] = &[
    RunRow {
        case: "metrics data, CSV, run not started",
        source_type: METRICS_DATA,
        data_format: CSV,
        run: Run::Created,
    },
    RunRow {
        case: "metrics data, JSON, run in progress",
        source_type: METRICS_DATA,
        data_format: JSON,
        run: Run::Processing,
    },
    RunRow {
        case: "metrics data, CSV, run finished",
        source_type: METRICS_DATA,
        data_format: CSV,
        run: Run::Completed,
    },
    RunRow {
        case: "message insights, JSON, run not started",
        source_type: MESSAGE_INSIGHTS,
        data_format: JSON,
        run: Run::Created,
    },
    RunRow {
        case: "message insights, CSV, run in progress",
        source_type: MESSAGE_INSIGHTS,
        data_format: CSV,
        run: Run::Processing,
    },
    RunRow {
        case: "message insights, JSON, run finished",
        source_type: MESSAGE_INSIGHTS,
        data_format: JSON,
        run: Run::Completed,
    },
];

#[tokio::test]
async fn a_read_reports_the_stored_job_and_its_destination_once_the_run_wrote_a_file() {
    for row in RUN_ROWS {
        let case = row.case;
        let ses = with_logged_day().await;
        let data_source = if row.source_type == METRICS_DATA {
            metrics_source(json!({"ISP": ["gmail"]}), json!([metric("SEND", "VOLUME")]))
        } else {
            insights_source(json!({}))
        };
        let job_id = ses.create(data_source.clone(), row.data_format).await;
        ses.drive(&job_id, row.run);

        let reply = ses.get(&job_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from the read, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("JobId"),
            job_id,
            "case {case:?}: the read must report the job it was asked for, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("ExportSourceType"),
            row.source_type,
            "case {case:?}: expected the source type the data source selects, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("JobStatus"),
            row.run.status(),
            "case {case:?}: expected the status the run has reached, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.data_format(),
            row.data_format,
            "case {case:?}: expected the data format the create asked for, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.member("ExportDataSource"),
            Some(data_source.clone()),
            "case {case:?}: the read must report the data source the create supplied, got {:?}",
            reply.member("ExportDataSource")
        );
        assert!(
            reply.member("CreatedTimestamp").is_some(),
            "case {case:?}: a stored job was created at a known instant, got {:?}",
            reply.body
        );
        assert!(
            reply.member("FailureInfo").is_none(),
            "case {case:?}: an emulated run cannot fail, got {:?}",
            reply.member("FailureInfo")
        );

        let finished = row.run == Run::Completed;
        assert_eq!(
            reply.s3_url().is_some(),
            finished,
            "case {case:?}: the pre-signed URL appears exactly when the run has \
             completed, got {:?}",
            reply.s3_url()
        );
        assert_eq!(
            reply.statistics().is_some(),
            finished,
            "case {case:?}: the statistics appear exactly when the run has written a \
             file, got {:?}",
            reply.statistics()
        );
        assert_eq!(
            reply.member("CompletedTimestamp").is_some(),
            finished,
            "case {case:?}: the completion instant appears exactly when the run has \
             completed, got {:?}",
            reply.member("CompletedTimestamp")
        );

        if let Some(url) = reply.s3_url() {
            let fetched = ses.fetch(&url).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: the URL the read handed out must serve the file, got {} {:?}",
                fetched.status, fetched.body
            );
            let expected_media_type = if row.data_format == JSON {
                "application/json"
            } else {
                "text/csv"
            };
            assert_eq!(
                fetched.media_type, expected_media_type,
                "case {case:?}: expected the media type the data format is served as, \
                 got {:?}",
                fetched.media_type
            );
        }
    }
}

/// One row of the metrics-file table.
struct MetricsRow {
    case: &'static str,
    /// `MetricsDataSource$Dimensions`, as the request carries it.
    dimensions: fn() -> Value,
    /// `MetricsDataSource$Metrics`, as the request carries it.
    metrics: fn() -> Value,
    /// The fields the file must carry, in order.
    fields: &'static [&'static str],
    /// The rows the file must carry, in order, as their cells.
    rows: &'static [&'static [&'static str]],
    /// `(ProcessedRecordsCount, ExportedRecordsCount)` the run must report.
    statistics: (i64, i64),
}

/// The metrics exports these cases ask for.
///
/// The rows differ along what the rule generalises over: how many dimensions the
/// request names and how many values each carries, which metrics and which
/// aggregations it asks for, and whether the activity it selects exists at all.
const METRICS_ROWS: &[MetricsRow] = &[
    MetricsRow {
        case: "one dimension with two values, a volume and a rate",
        dimensions: || json!({"ISP": ["gmail", "yahoo"]}),
        metrics: || {
            json!([
                metric("SEND", "VOLUME"),
                metric("SEND", "RATE"),
                metric("COMPLAINT", "VOLUME"),
            ])
        },
        fields: &["Date", "ISP", "SEND_VOLUME", "SEND_RATE", "COMPLAINT_VOLUME"],
        rows: &[
            &[DAY, "gmail", "2", "1.000000", "0"],
            &[DAY, "yahoo", "1", "1.000000", "0"],
        ],
        statistics: (3, 2),
    },
    MetricsRow {
        case: "two dimensions, one combination of which sent nothing",
        dimensions: || json!({"ISP": ["gmail"], "EMAIL_IDENTITY": ["sender@example.com", "other@example.com"]}),
        metrics: || json!([metric("SEND", "VOLUME")]),
        fields: &["Date", "EMAIL_IDENTITY", "ISP", "SEND_VOLUME"],
        // The identity dimension keeps only the messages the identity it names
        // authorised, so the other identity — which sent to another provider — holds
        // no activity for this combination and is not a record.
        rows: &[&[DAY, "sender@example.com", "gmail", "2"]],
        statistics: (3, 1),
    },
    MetricsRow {
        case: "a dimension value nothing was sent to",
        dimensions: || json!({"ISP": ["outlook"]}),
        metrics: || json!([metric("SEND", "VOLUME")]),
        fields: &["Date", "ISP", "SEND_VOLUME"],
        rows: &[],
        statistics: (3, 0),
    },
    MetricsRow {
        case: "a metric the request names no aggregation for",
        dimensions: || json!({"ISP": ["yahoo"]}),
        metrics: || json!([{"Name": "SEND"}]),
        fields: &["Date", "ISP", "SEND_VOLUME"],
        rows: &[&[DAY, "yahoo", "1"]],
        statistics: (3, 1),
    },
];

#[tokio::test]
async fn a_metrics_export_file_derives_its_shape_from_the_request_and_its_values_from_the_records()
{
    for row in METRICS_ROWS {
        for data_format in [CSV, JSON] {
            let case = format!("{} as {data_format}", row.case);
            let ses = with_logged_day().await;
            let job_id = ses
                .create(
                    metrics_source((row.dimensions)(), (row.metrics)()),
                    data_format,
                )
                .await;
            ses.drive(&job_id, Run::Completed);

            let reply = ses.get(&job_id).await;
            assert_eq!(
                reply.statistics(),
                Some(row.statistics),
                "case {case:?}: expected the counts of the records the run read and \
                 wrote, got {:?}",
                reply.statistics()
            );
            let url = reply.s3_url().unwrap_or_else(|| {
                panic!("case {case:?}: a completed run must hand out a URL, got {:?}", reply.body)
            });
            let fetched = ses.fetch(&url).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: expected the file, got {} {:?}",
                fetched.status, fetched.body
            );

            if data_format == CSV {
                let (header, rows) = fetched.csv_table();
                assert_eq!(
                    header, row.fields,
                    "case {case:?}: expected the fields the request's dimensions and \
                     metrics name, got {header:?}"
                );
                let expected: Vec<Vec<String>> = row
                    .rows
                    .iter()
                    .map(|cells| cells.iter().map(|cell| (*cell).to_string()).collect())
                    .collect();
                assert_eq!(
                    rows, expected,
                    "case {case:?}: expected one record per day the records hold \
                     activity on per dimension combination, got {rows:?}"
                );
            } else {
                let records = fetched.json_records();
                assert_eq!(
                    records.len(),
                    row.rows.len(),
                    "case {case:?}: expected the same records as the CSV rendering, \
                     got {records:?}"
                );
                for (record, cells) in records.iter().zip(row.rows) {
                    for (field, cell) in row.fields.iter().zip(*cells) {
                        let carried = record.get(*field).cloned().unwrap_or(Value::Null);
                        let shown = match &carried {
                            Value::String(text) => text.clone(),
                            Value::Number(number) => number.to_string(),
                            other => other.to_string(),
                        };
                        let matches = match carried {
                            // A rate is a number in JSON and a fixed-precision
                            // decimal in CSV, so the two are compared as numbers.
                            Value::Number(ref number) => {
                                number.as_f64().unwrap_or_default()
                                    == cell.parse::<f64>().unwrap_or_default()
                            }
                            _ => shown == *cell,
                        };
                        assert!(
                            matches,
                            "case {case:?}: expected {field} {cell:?}, got {shown:?}"
                        );
                    }
                }
            }
        }
    }
}

/// One row of the Message Insights filter table.
struct InsightsRow {
    case: &'static str,
    /// The `Include`, `Exclude` and `MaxResults` members the request carries.
    filters: fn() -> Value,
    /// The recipients the file must hold a record for, in order.
    destinations: &'static [&'static str],
    /// `(ProcessedRecordsCount, ExportedRecordsCount)` the run must report.
    statistics: (i64, i64),
}

/// The Message Insights exports these cases ask for.
///
/// The rows differ along what the rule generalises over: each filter member the model
/// documents (`smithy_query(query="shape",shape="MessageInsightsFilters")`), the
/// partial match its `*` wildcard admits at either end, whether the filter includes or
/// excludes, and the result limit.
const INSIGHTS_ROWS: &[InsightsRow] = &[
    InsightsRow {
        case: "no filter at all",
        filters: || json!({}),
        destinations: &["first@gmail.com", "second@gmail.com", "third@yahoo.com"],
        statistics: (3, 3),
    },
    InsightsRow {
        case: "a destination suffix match",
        filters: || json!({"Include": {"Destination": ["*@gmail.com"]}}),
        destinations: &["first@gmail.com", "second@gmail.com"],
        statistics: (3, 2),
    },
    InsightsRow {
        case: "a subject prefix match",
        filters: || json!({"Include": {"Subject": ["Weekly*"]}}),
        destinations: &["first@gmail.com", "third@yahoo.com"],
        statistics: (3, 2),
    },
    InsightsRow {
        case: "an exact from address, of two values joined by OR",
        filters: || json!({"Include": {"FromEmailAddress": ["other@example.com", "nobody@example.com"]}}),
        destinations: &["third@yahoo.com"],
        statistics: (3, 1),
    },
    InsightsRow {
        case: "two filters joined by AND",
        filters: || {
            json!({"Include": {"Destination": ["*gmail*"], "Subject": ["*digest"]}})
        },
        destinations: &["first@gmail.com"],
        statistics: (3, 1),
    },
    InsightsRow {
        case: "an excluded provider, named without its public suffix",
        filters: || json!({"Exclude": {"Isp": ["Gmail"]}}),
        destinations: &["third@yahoo.com"],
        statistics: (3, 1),
    },
    InsightsRow {
        case: "the delivery event every accepted message carries",
        filters: || json!({"Include": {"LastDeliveryEvent": ["SEND"]}}),
        destinations: &["first@gmail.com", "second@gmail.com", "third@yahoo.com"],
        statistics: (3, 3),
    },
    InsightsRow {
        case: "a result limit under what the interval holds",
        filters: || json!({"MaxResults": 2}),
        destinations: &["first@gmail.com", "second@gmail.com"],
        statistics: (3, 2),
    },
];

#[tokio::test]
async fn a_message_insights_export_file_holds_the_records_its_filters_select() {
    /// The field a record carries its recipient in.
    const DESTINATION: &str = "Destination";

    for row in INSIGHTS_ROWS {
        let case = row.case;
        let ses = with_logged_day().await;
        let job_id = ses.create(insights_source((row.filters)()), CSV).await;
        ses.drive(&job_id, Run::Completed);

        let reply = ses.get(&job_id).await;
        assert_eq!(
            reply.statistics(),
            Some(row.statistics),
            "case {case:?}: expected the counts of the records the run read and wrote, \
             got {:?}",
            reply.statistics()
        );
        let url = reply.s3_url().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a completed run must hand out a URL, got {:?}",
                reply.body
            )
        });
        let fetched = ses.fetch(&url).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the file, got {} {:?}",
            fetched.status, fetched.body
        );
        let (header, rows) = fetched.csv_table();
        let at = header
            .iter()
            .position(|field| field == DESTINATION)
            .unwrap_or_else(|| panic!("case {case:?}: expected a {DESTINATION} field, got {header:?}"));
        let destinations: Vec<String> = rows
            .iter()
            .map(|cells| cells.get(at).cloned().unwrap_or_default())
            .collect();
        assert_eq!(
            destinations, row.destinations,
            "case {case:?}: expected one record per recipient the filters keep, got \
             {destinations:?}"
        );
        for cells in &rows {
            let field = |name: &str| {
                header
                    .iter()
                    .position(|carried| carried == name)
                    .and_then(|at| cells.get(at).cloned())
                    .unwrap_or_default()
            };
            assert_eq!(
                field("LastDeliveryEvent"),
                "SEND",
                "case {case:?}: the acceptance is the last delivery event of every \
                 logged message, got {cells:?}"
            );
            assert!(
                !field("MessageId").is_empty() && !field("Timestamp").is_empty(),
                "case {case:?}: every record names its message and when it was \
                 accepted, got {cells:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_value_escapes_the_wildcard_to_match_it_literally() {
    /// The subjects the escaped cases select over: one that IS the wildcard text, one
    /// that carries it, and one that carries neither.
    const SUBJECTS: [(&str, &str); 3] = [
        ("exact@gmail.com", "50*"),
        ("inside@gmail.com", "Sale 50* today"),
        ("neither@gmail.com", "50 percent off"),
    ];

    let ses = Ses::new();
    for (index, (to, subject)) in SUBJECTS.iter().enumerate() {
        ses.log(LoggedMessage {
            from: "sender@example.com",
            to: vec![to],
            cc: Vec::new(),
            subject: Some(subject),
            at: DAY_START + 100 * (index as i64 + 1),
        });
    }

    // "In order to match the literal characters `*` or `\`, they must be escaped using
    // the `\` character": an escaped star is text to match, an unescaped one is the
    // wildcard.
    for (case, pattern, expected) in [
        (
            "an escaped star, so an exact match on the wildcard text",
            r"50\*",
            vec!["exact@gmail.com"],
        ),
        (
            "an escaped star between two wildcards, so a contains match",
            r"*50\**",
            vec!["exact@gmail.com", "inside@gmail.com"],
        ),
        (
            "an unescaped trailing star, so a prefix match",
            "50*",
            vec!["exact@gmail.com", "neither@gmail.com"],
        ),
    ] {
        let job_id = ses
            .create(
                insights_source(json!({"Include": {"Subject": [pattern]}})),
                CSV,
            )
            .await;
        ses.drive(&job_id, Run::Completed);
        let url = ses
            .get(&job_id)
            .await
            .s3_url()
            .unwrap_or_else(|| panic!("case {case:?}: a completed run must hand out a URL"));
        let fetched = ses.fetch(&url).await;
        let (header, rows) = fetched.csv_table();
        let at = header
            .iter()
            .position(|field| field == "Destination")
            .unwrap_or_else(|| panic!("case {case:?}: expected a Destination field, got {header:?}"));
        let destinations: Vec<String> = rows
            .iter()
            .map(|cells| cells.get(at).cloned().unwrap_or_default())
            .collect();
        assert_eq!(
            destinations, expected,
            "case {case:?}: expected the records the pattern {pattern:?} selects, got \
             {destinations:?}"
        );
    }
}

#[tokio::test]
async fn a_cell_that_carries_the_separators_is_quoted_by_the_codec() {
    /// A subject carrying both of the characters the comma-separated form has to
    /// quote: the separator itself and the quote character.
    const SUBJECT: &str = r#"Receipt, "paid" in full"#;

    let ses = Ses::new();
    ses.log(LoggedMessage::titled(
        "sender@example.com",
        "buyer@gmail.com",
        SUBJECT,
        DAY_START + 500,
    ));
    let job_id = ses.create(insights_source(json!({})), CSV).await;
    ses.drive(&job_id, Run::Completed);
    let url = ses
        .get(&job_id)
        .await
        .s3_url()
        .expect("a completed run hands out a URL");
    let fetched = ses.fetch(&url).await;

    // The field is quoted and each quote inside it doubled, so a reader splitting the
    // row on its separators reads the subject back whole.
    let quoted = format!("\"{}\"", SUBJECT.replace('"', "\"\""));
    assert!(
        fetched.body.contains(&quoted),
        "expected the subject to be written as {quoted}, got {:?}",
        fetched.body
    );
}

#[tokio::test]
async fn an_export_file_is_served_only_to_a_request_made_with_the_signed_url() {
    let ses = with_logged_day().await;
    let job_id = ses.create(insights_source(json!({})), CSV).await;
    ses.drive(&job_id, Run::Completed);
    let signed = ses
        .get(&job_id)
        .await
        .s3_url()
        .expect("a completed run hands out a URL");
    let path = signed
        .split_once('?')
        .map(|(address, _)| address.to_string())
        .unwrap_or_else(|| signed.clone());

    // The same path, addressed by an API caller rather than through the URL this
    // emulator signed: nothing is served from it, because the file's only address is a
    // pre-signed URL for that one job. The answer is the one this service gives any
    // path its model does not bind — the file's path is not an operation.
    let unsigned = ses.call("GET", path_of(&path), None).await;
    let unbound = ses
        .call("GET", "/v2/email/export-jobs/not-a-route-at-all/x", None)
        .await;
    assert_ne!(
        unsigned.status, 200,
        "case \"the path without the signed query\": nothing may be served, got {:?}",
        unsigned.body
    );
    assert_eq!(
        unsigned.status, unbound.status,
        "case \"the path without the signed query\": expected the answer this service \
         gives an unbound path ({}), got {}",
        unbound.status, unsigned.status
    );
    assert!(
        !unsigned.body.to_string().contains("MessageId"),
        "case \"the path without the signed query\": no part of the file may be served, \
         got {:?}",
        unsigned.body
    );

    let fetched = ses.fetch(&signed).await;
    assert_eq!(
        fetched.status, 200,
        "case \"the signed URL\": expected the file, got {} {:?}",
        fetched.status, fetched.body
    );
}

#[tokio::test]
async fn one_call_writes_at_most_the_export_files_it_is_budgeted_for() {
    let ses = with_logged_day().await;
    let pending = MAX_EXPORT_FILES_PER_CALL + 2;
    let mut job_ids = Vec::new();
    for _ in 0..pending {
        let job_id = ses.create(insights_source(json!({})), CSV).await;
        ses.drive(&job_id, Run::Completed);
        job_ids.push(job_id);
    }
    // Creating the last job settled the store once already, so count from a clean
    // slate: every run's completion is due, and none of their files is written yet.
    ses.clear_files();

    for (case, expected_files) in [
        ("after one call", MAX_EXPORT_FILES_PER_CALL),
        ("after a second call", pending),
    ] {
        // One API call settles the store once, which is what the budget bounds.
        let reply = ses.get(&job_ids[0]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the read must succeed, got {} {:?}",
            reply.status, reply.body
        );
        let written = ses.jobs_with_a_file();
        assert_eq!(
            written, expected_files,
            "case {case:?}: expected {expected_files} of the {pending} due runs to have \
             written their file, got {written}"
        );
        // A run whose file is not written is not complete, so nothing reports a
        // finished run with nothing to download.
        assert_eq!(
            ses.completed_jobs(),
            written,
            "case {case:?}: a run is complete exactly when its file is written, got \
             {} complete and {written} written",
            ses.completed_jobs()
        );
    }
}

#[tokio::test]
async fn a_job_id_the_account_holds_no_export_job_for_is_not_found() {
    let ses = with_logged_day().await;
    let stored = ses
        .create(insights_source(json!({})), CSV)
        .await;
    for case in [
        "an id no create ever minted",
        "the stored id with one character more",
    ] {
        let job_id = if case.starts_with("an id no") {
            "00000000-0000-4000-8000-000000000000".to_string()
        } else {
            format!("{stored}0")
        };
        let reply = ses.get(&job_id).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NotFoundException",
            "case {case:?}: expected the modelled not-found error, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("Export job {job_id} does not exist."),
            "case {case:?}: expected the not-found wording to name the requested id, \
             got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_download_url_serves_nothing_for_a_job_whose_run_wrote_no_file() {
    let ses = with_logged_day().await;
    // The URL of a completed job, which is the only way to get a signed one, then the
    // same URL for jobs whose run has not written a file.
    let finished = ses.create(insights_source(json!({})), CSV).await;
    ses.drive(&finished, Run::Completed);
    let served = ses
        .get(&finished)
        .await
        .s3_url()
        .expect("a completed run hands out a URL");

    let unfinished = ses.create(insights_source(json!({})), CSV).await;
    ses.drive(&unfinished, Run::Processing);
    for (case, job_id) in [
        ("a run still in progress", unfinished.as_str()),
        ("a job the account does not hold", "no-such-job"),
    ] {
        let url = served.replace(&finished, job_id);
        let fetched = ses.fetch(&url).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected no file at that location, got {} {:?}",
            fetched.status, fetched.body
        );
    }

    let fetched = ses.fetch(&served).await;
    assert_eq!(
        fetched.status, 200,
        "case \"the completed job's own URL\": expected the file, got {} {:?}",
        fetched.status, fetched.body
    );
}

#[tokio::test]
async fn the_file_a_run_wrote_does_not_change_when_the_records_do() {
    let ses = with_logged_day().await;
    let job_id = ses.create(insights_source(json!({})), CSV).await;
    ses.drive(&job_id, Run::Completed);
    let first = ses.get(&job_id).await;
    let url = first.s3_url().expect("a completed run hands out a URL");
    let before = ses.fetch(&url).await.body;

    // A message accepted after the run completed is not part of the file it wrote: the
    // export is the records as they stood when the run fell due.
    ses.log(LoggedMessage::titled(
        "sender@example.com",
        "after@gmail.com",
        "Later",
        DAY_START + 20_000,
    ));
    let again = ses.get(&job_id).await;
    let after = ses.fetch(&url).await.body;
    assert_eq!(
        after, before,
        "the file a completed run wrote must not change when the records do, got \
         {after:?}"
    );
    assert_eq!(
        again.statistics(),
        first.statistics(),
        "the statistics of a completed run count the records it wrote, got {:?}",
        again.statistics()
    );
}
