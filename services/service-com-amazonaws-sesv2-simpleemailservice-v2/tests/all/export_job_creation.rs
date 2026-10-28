// SPDX-License-Identifier: CC0-1.0

//! Creating an export job (`CreateExportJob`), end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reads the stored job back both through the engine's own state and
//! through the implemented reader of the account's export jobs (`ListExportJobs`).
//!
//! The rules these cases pin:
//!
//! * a create stores one new export job and answers the `JobId` minted for it, which is
//!   the only name the job has: the stored job carries the whole data source and the
//!   data format the request supplied, its run is in the status a job is created in, and
//!   both of its clock-owed transitions are due at the spans the state module states;
//! * the job's `ExportSourceType` is decided by WHICH member of `ExportDataSource` the
//!   request carries, not by anything else in it;
//! * `ExportDataSource` carries exactly one of its two members, so a request naming
//!   neither or both is refused with the one input error the operation models, and
//!   persists nothing;
//! * `ExportDestination$S3Url` is SES's to set, so a create that carries one is refused
//!   the same way and persists nothing;
//! * the created job is reported by a read of the account's export jobs, and the
//!   `ExportSourceType` filter partitions the jobs a run of these cases created.
//!
//! The table below spans what the rules generalise over: both members of
//! `ExportDataSource`, both `DataFormat` values, one and the maximum of three metric
//! dimensions, one and several metrics with both aggregations, a Message Insights source
//! with and without filters, and `MaxResults` at each published bound.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    DATA_FORMAT_VALUES, EXPORT_SOURCE_TYPE_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    export_destination_s3_url_is_set_by_ses, invalid_export_data_source, ExportJobRecord,
    ComAmazonawsSesv2SimpleemailserviceV2State, EXPORT_JOB_COMPLETION_SETTLE,
    EXPORT_JOB_PROCESSING_SETTLE, MAX_EXPORT_JOBS,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An export job belongs to the account, so one Region is enough.
const HOME: &str = "us-east-1";

/// `ExportSourceType$METRICS_DATA` — the source type a metrics data source selects.
const METRICS_DATA: &str = "METRICS_DATA";

/// `ExportSourceType$MESSAGE_INSIGHTS` — the source type a Message Insights data source
/// selects.
const MESSAGE_INSIGHTS: &str = "MESSAGE_INSIGHTS";

/// The published bounds of `MessageInsightsDataSource$MaxResults`
/// (`smithy_query(query="shape",shape="MessageInsightsExportMaxResults")`: range 1 to
/// 10000).
const MAX_RESULTS_BOUNDS: [i64; 2] = [1, 10_000];

/// The start of the date range the cases export, as whole epoch seconds so the instant
/// the stored source carries compares exactly.
const RANGE_START_SECS: i64 = 1_761_580_800;

/// The end of that range: a day later.
const RANGE_END_SECS: i64 = RANGE_START_SECS + 24 * 60 * 60;

/// How far apart the instant a create is answered at and the instant the assertion
/// reads may be, so a case cannot fail on the wall clock moving between the two.
const CLOCK_SLACK: Duration = Duration::from_secs(30);

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
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

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `JobId` the create answered with, or an empty string when it carried none.
    fn job_id(&self) -> String {
        self.body
            .get("JobId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The summaries a listing reported, in the order it reported them.
    fn jobs(&self) -> Vec<Value> {
        self.body
            .get("ExportJobs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The summary the listing reported for `job_id`, if it reported one.
    fn job(&self, job_id: &str) -> Option<Value> {
        self.jobs()
            .into_iter()
            .find(|job| job.get("JobId").and_then(Value::as_str) == Some(job_id))
    }
}

/// One create request the cases drive, and what the job it stores must carry.
struct Row {
    case: &'static str,
    /// The `ExportDataSource` document the request carries.
    data_source: Value,
    /// `ExportDestination$DataFormat`.
    data_format: &'static str,
    /// The `ExportSourceType` the data source selects.
    source_type: &'static str,
}

/// The create requests the cases drive.
///
/// The rows span what the rules generalise over: both members of `ExportDataSource`,
/// both `DataFormat` values (`smithy_query(query="shape",shape="DataFormat")`), one and
/// the maximum of three `ExportDimensions` entries with dimension names drawn from
/// `MetricDimensionName`, one and several `ExportMetrics` entries drawn from `Metric`
/// with both `MetricAggregation` values, a Message Insights source with no filters and
/// one with both `Include` and `Exclude`, and `MaxResults` at each published bound.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "metrics data, one dimension, one metric, CSV",
            data_source: json!({
                "MetricsDataSource": {
                    "Dimensions": { "EMAIL_IDENTITY": ["sender.example.com"] },
                    "Namespace": "VDM",
                    "Metrics": [{ "Name": "SEND", "Aggregation": "VOLUME" }],
                    "StartDate": RANGE_START_SECS,
                    "EndDate": RANGE_END_SECS,
                }
            }),
            data_format: "CSV",
            source_type: METRICS_DATA,
        },
        Row {
            case: "metrics data, the maximum three dimensions, several metrics, JSON",
            data_source: json!({
                "MetricsDataSource": {
                    "Dimensions": {
                        "EMAIL_IDENTITY": ["sender.example.com", "alerts.example.com"],
                        "CONFIGURATION_SET": ["campaign"],
                        "ISP": ["example-isp"],
                    },
                    "Namespace": "VDM",
                    "Metrics": [
                        { "Name": "DELIVERY", "Aggregation": "VOLUME" },
                        { "Name": "OPEN", "Aggregation": "RATE" },
                        { "Name": "PERMANENT_BOUNCE", "Aggregation": "RATE" },
                    ],
                    "StartDate": RANGE_START_SECS,
                    "EndDate": RANGE_END_SECS,
                }
            }),
            data_format: "JSON",
            source_type: METRICS_DATA,
        },
        Row {
            case: "message insights, no filters, CSV",
            data_source: json!({
                "MessageInsightsDataSource": {
                    "StartDate": RANGE_START_SECS,
                    "EndDate": RANGE_END_SECS,
                }
            }),
            data_format: "CSV",
            source_type: MESSAGE_INSIGHTS,
        },
        Row {
            case: "message insights, both filter sets, MaxResults at the published minimum, JSON",
            data_source: json!({
                "MessageInsightsDataSource": {
                    "StartDate": RANGE_START_SECS,
                    "EndDate": RANGE_END_SECS,
                    "Include": {
                        "FromEmailAddress": ["sender@sender.example.com"],
                        "LastDeliveryEvent": ["DELIVERY"],
                    },
                    "Exclude": {
                        "Destination": ["recipient@example.com"],
                        "LastEngagementEvent": ["CLICK"],
                    },
                    "MaxResults": MAX_RESULTS_BOUNDS[0],
                }
            }),
            data_format: "JSON",
            source_type: MESSAGE_INSIGHTS,
        },
        Row {
            case: "message insights, MaxResults at the published maximum, CSV",
            data_source: json!({
                "MessageInsightsDataSource": {
                    "StartDate": RANGE_START_SECS,
                    "EndDate": RANGE_END_SECS,
                    "MaxResults": MAX_RESULTS_BOUNDS[1],
                }
            }),
            data_format: "CSV",
            source_type: MESSAGE_INSIGHTS,
        },
    ]
}

/// The emulated service behind its router, plus the shared engine so a case can read the
/// stored job back.
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

    /// Send one request with `body` as its document to `path`.
    async fn post(&self, path: &str, body: Value) -> Reply {
        let payload = bytes::Bytes::from(serde_json::to_vec(&body).expect("serialize body"));
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
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

    /// Send one `CreateExportJob` request.
    async fn create(&self, body: Value) -> Reply {
        self.post("/v2/email/export-jobs", body).await
    }

    /// Create the job `row` describes, and answer the id minted for it.
    async fn create_row(&self, row: &Row) -> String {
        let reply = self
            .create(json!({
                "ExportDataSource": row.data_source,
                "ExportDestination": { "DataFormat": row.data_format },
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: a create of a modelled data source is served, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let job_id = reply.job_id();
        assert!(
            !job_id.is_empty(),
            "case {:?}: expected the minted JobId, got {:?}",
            row.case,
            reply.body
        );
        job_id
    }

    /// Send one `ListExportJobs` request.
    async fn list(&self, body: Value) -> Reply {
        let reply = self.post("/v2/email/list-export-jobs", body).await;
        assert_eq!(
            reply.status, 200,
            "the listing must succeed, got {} {:?}",
            reply.status, reply.body
        );
        reply
    }

    /// The stored job `job_id` names, if the store holds one.
    fn stored(&self, job_id: &str) -> Option<ExportJobRecord> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.export_jobs.get(job_id).cloned()
    }

    /// How many export jobs the account holds.
    fn stored_count(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.export_jobs.len()
    }

    /// Fill the store with `count` export jobs, as creates of them would leave it.
    fn fill(&self, count: usize) {
        let now = SystemTime::now();
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        for index in 0..count {
            let job_id = format!("seeded-{index:08}");
            store.export_jobs.insert(
                job_id.clone(),
                ExportJobRecord::new(job_id, METRICS_DATA, now),
            );
        }
    }
}

#[tokio::test]
async fn a_create_stores_the_job_it_mints_and_answers_its_job_id() {
    // The formats the rows span are the modelled set, so no value of `DataFormat` goes
    // untested.
    assert_eq!(
        DATA_FORMAT_VALUES,
        &["CSV", "JSON"],
        "the data formats these cases span must be the modelled set, got {DATA_FORMAT_VALUES:?}"
    );

    let ses = Ses::new();
    let mut minted: Vec<String> = Vec::new();

    for row in rows() {
        let case = row.case;
        let before = SystemTime::now();
        let job_id = ses.create_row(&row).await;
        let after = SystemTime::now();

        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every create mints a job id of its own, got {job_id:?} again"
        );
        minted.push(job_id.clone());

        let record = ses
            .stored(&job_id)
            .unwrap_or_else(|| panic!("case {case:?}: the create must store a job under {job_id:?}"));
        assert_eq!(
            record.job_id, job_id,
            "case {case:?}: the stored job carries the id the create answered with, got {:?}",
            record.job_id
        );
        assert_eq!(
            record.source_type, row.source_type,
            "case {case:?}: expected the source type the data source selects, got {:?}",
            record.source_type
        );
        assert_eq!(
            record.data_format, row.data_format,
            "case {case:?}: expected the data format the request named, got {:?}",
            record.data_format
        );
        assert_eq!(
            record.status.as_str(),
            "CREATED",
            "case {case:?}: a job has just been created, got {:?}",
            record.status
        );
        assert_eq!(
            record.completed_at, None,
            "case {case:?}: a run that has not finished has no completion instant, got {:?}",
            record.completed_at
        );
        assert!(
            record.created_at >= before - CLOCK_SLACK && record.created_at <= after + CLOCK_SLACK,
            "case {case:?}: expected the job created at the instant of the call, got {:?}",
            record.created_at
        );

        // Both transitions of the run are owed by the clock at the spans the state
        // module states, counted from when the job was created.
        assert_eq!(
            record
                .processing_due_at
                .duration_since(record.created_at)
                .ok(),
            Some(EXPORT_JOB_PROCESSING_SETTLE),
            "case {case:?}: expected the start of the run due at the stated span, got {:?}",
            record.processing_due_at
        );
        assert_eq!(
            record
                .completion_due_at
                .duration_since(record.created_at)
                .ok(),
            Some(EXPORT_JOB_PROCESSING_SETTLE + EXPORT_JOB_COMPLETION_SETTLE),
            "case {case:?}: expected the completion of the run due at the stated span, got {:?}",
            record.completion_due_at
        );

        // The data source is stored as supplied: the member the request carried is the
        // one the job holds, with the range and the detail it named.
        let metrics = record.data_source.metrics_data_source.as_ref();
        let insights = record.data_source.message_insights_data_source.as_ref();
        let supplied = &row.data_source;
        if row.source_type == METRICS_DATA {
            let stored = metrics
                .unwrap_or_else(|| panic!("case {case:?}: expected the stored metrics source"));
            assert!(
                insights.is_none(),
                "case {case:?}: the job holds only the member the request carried, got {insights:?}"
            );
            let wanted = &supplied["MetricsDataSource"];
            assert_eq!(
                stored.namespace,
                wanted["Namespace"].as_str().unwrap_or_default(),
                "case {case:?}: expected the namespace the request named, got {:?}",
                stored.namespace
            );
            let wanted_dimensions = wanted["Dimensions"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: the row names dimensions"));
            assert_eq!(
                stored.dimensions.len(),
                wanted_dimensions.len(),
                "case {case:?}: expected every dimension the request named, got {:?}",
                stored.dimensions
            );
            for (name, values) in wanted_dimensions {
                let stored_values = stored
                    .dimensions
                    .get(name)
                    .unwrap_or_else(|| panic!("case {case:?}: expected dimension {name:?}"));
                let wanted_values: Vec<String> = values
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect();
                assert_eq!(
                    stored_values, &wanted_values,
                    "case {case:?}: expected dimension {name:?} as supplied, got {stored_values:?}"
                );
            }
            let wanted_metrics = wanted["Metrics"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the row names metrics"));
            assert_eq!(
                stored.metrics.len(),
                wanted_metrics.len(),
                "case {case:?}: expected every metric the request named, got {:?}",
                stored.metrics
            );
            for (index, wanted_metric) in wanted_metrics.iter().enumerate() {
                let stored_metric = &stored.metrics[index];
                assert_eq!(
                    stored_metric.name.as_deref(),
                    wanted_metric["Name"].as_str(),
                    "case {case:?}: expected metric {index} as supplied, got {stored_metric:?}"
                );
                assert_eq!(
                    stored_metric.aggregation.as_deref(),
                    wanted_metric["Aggregation"].as_str(),
                    "case {case:?}: expected metric {index}'s aggregation as supplied, got {stored_metric:?}"
                );
            }
            assert_eq!(
                (stored.start_date.secs(), stored.end_date.secs()),
                (RANGE_START_SECS, RANGE_END_SECS),
                "case {case:?}: expected the date range the request named, got {:?}",
                (stored.start_date, stored.end_date)
            );
        } else {
            let stored = insights
                .unwrap_or_else(|| panic!("case {case:?}: expected the stored insights source"));
            assert!(
                metrics.is_none(),
                "case {case:?}: the job holds only the member the request carried, got {metrics:?}"
            );
            let wanted = &supplied["MessageInsightsDataSource"];
            assert_eq!(
                (stored.start_date.secs(), stored.end_date.secs()),
                (RANGE_START_SECS, RANGE_END_SECS),
                "case {case:?}: expected the date range the request named, got {:?}",
                (stored.start_date, stored.end_date)
            );
            assert_eq!(
                stored.max_results.map(i64::from),
                wanted["MaxResults"].as_i64(),
                "case {case:?}: expected MaxResults as supplied, got {:?}",
                stored.max_results
            );
            assert_eq!(
                stored
                    .include
                    .as_ref()
                    .and_then(|filters| filters.from_email_address.clone()),
                wanted["Include"]["FromEmailAddress"].as_array().map(|values| values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<String>>()),
                "case {case:?}: expected the Include filter as supplied, got {:?}",
                stored.include
            );
            assert_eq!(
                stored
                    .exclude
                    .as_ref()
                    .and_then(|filters| filters.destination.clone()),
                wanted["Exclude"]["Destination"].as_array().map(|values| values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<String>>()),
                "case {case:?}: expected the Exclude filter as supplied, got {:?}",
                stored.exclude
            );
        }

        // The round trip through the implemented reader of the account's export jobs:
        // the job the create stored is reported as its record projects it.
        let listed = ses.list(json!({})).await;
        let summary = listed
            .job(&job_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {job_id:?} in {:?}", listed.body));
        assert_eq!(
            summary.get("ExportSourceType").and_then(Value::as_str),
            Some(row.source_type),
            "case {case:?}: expected the stored source type, got {summary:?}"
        );
        assert_eq!(
            summary.get("JobStatus").and_then(Value::as_str),
            Some("CREATED"),
            "case {case:?}: expected the status the job was created in, got {summary:?}"
        );
        assert!(
            summary.get("CreatedTimestamp").is_some(),
            "case {case:?}: expected the instant the job was created, got {summary:?}"
        );
        assert_eq!(
            summary.get("CompletedTimestamp"),
            None,
            "case {case:?}: a run that has not finished has no completion instant, got {summary:?}"
        );
    }

    assert_eq!(
        ses.stored_count(),
        minted.len(),
        "every create stores exactly one job, so the account holds one per call, got {}",
        ses.stored_count()
    );
}

#[tokio::test]
async fn the_source_type_filter_partitions_the_jobs_the_creates_made() {
    assert_eq!(
        EXPORT_SOURCE_TYPE_VALUES,
        &["MESSAGE_INSIGHTS", "METRICS_DATA"],
        "the source types these cases span must be the modelled set, got {EXPORT_SOURCE_TYPE_VALUES:?}"
    );

    let ses = Ses::new();
    let mut created: Vec<(&'static str, String)> = Vec::new();
    for row in rows() {
        let job_id = ses.create_row(&row).await;
        created.push((row.source_type, job_id));
    }

    for wanted in EXPORT_SOURCE_TYPE_VALUES {
        let reply = ses.list(json!({ "ExportSourceType": wanted })).await;
        let mut reported: Vec<String> = reply
            .jobs()
            .iter()
            .map(|job| {
                job.get("JobId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        reported.sort();
        let mut expected: Vec<String> = created
            .iter()
            .filter(|(source_type, _)| source_type == wanted)
            .map(|(_, job_id)| job_id.clone())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {wanted:?}: expected exactly the created jobs of that source type, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_create_past_the_ceiling_on_the_store_is_refused() {
    let row = &rows()[0];

    // One short of the ceiling the store is still open, so a create is served.
    let ses = Ses::new();
    ses.fill(MAX_EXPORT_JOBS - 1);
    let job_id = ses.create_row(row).await;
    assert!(
        ses.stored(&job_id).is_some(),
        "case {:?}: a create below the ceiling stores its job, got {} stored jobs",
        row.case,
        ses.stored_count()
    );

    // At the ceiling the account holds all the export jobs it may, so the next create
    // is refused with the error the model binds for it and stores nothing.
    let full = Ses::new();
    full.fill(MAX_EXPORT_JOBS);
    let reply = full
        .create(json!({
            "ExportDataSource": row.data_source,
            "ExportDestination": { "DataFormat": row.data_format },
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case {:?}: expected the limit refusal, got {} {:?}",
        row.case, reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "LimitExceededException",
        "case {:?}: expected the error the model binds for too many of a resource, got {:?}",
        row.case,
        reply.body
    );
    assert!(
        reply.message().contains(&MAX_EXPORT_JOBS.to_string()),
        "case {:?}: expected the ceiling in the message, got {:?}",
        row.case,
        reply.message()
    );
    assert_eq!(
        full.stored_count(),
        MAX_EXPORT_JOBS,
        "a refused create stores nothing, got {} stored jobs",
        full.stored_count()
    );
}

#[tokio::test]
async fn a_data_source_that_selects_no_one_source_is_refused_and_persists_nothing() {
    let ses = Ses::new();
    let both = json!({
        "MetricsDataSource": {
            "Dimensions": { "ISP": ["example-isp"] },
            "Namespace": "VDM",
            "Metrics": [{ "Name": "SEND", "Aggregation": "VOLUME" }],
            "StartDate": RANGE_START_SECS,
            "EndDate": RANGE_END_SECS,
        },
        "MessageInsightsDataSource": {
            "StartDate": RANGE_START_SECS,
            "EndDate": RANGE_END_SECS,
        }
    });
    let cases: Vec<(&str, Value)> = vec![
        ("an ExportDataSource naming neither member", json!({})),
        ("an ExportDataSource naming both members", both),
    ];

    for (case, data_source) in cases {
        for data_format in DATA_FORMAT_VALUES {
            let reply = ses
                .create(json!({
                    "ExportDataSource": data_source,
                    "ExportDestination": { "DataFormat": data_format },
                }))
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({data_format}): expected the input refusal, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case:?} ({data_format}): expected the operation's input error, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.message(),
                invalid_export_data_source(),
                "case {case:?} ({data_format}): expected the modelled rule as the message, got {:?}",
                reply.message()
            );
        }
    }

    assert_eq!(
        ses.stored_count(),
        0,
        "a refused create stores nothing, got {} stored jobs",
        ses.stored_count()
    );
}

#[tokio::test]
async fn a_create_that_carries_the_destination_url_is_refused_and_persists_nothing() {
    let ses = Ses::new();
    // Every value matches `ExportDestination$S3Url`'s modelled pattern, so each case is
    // refused by this rule rather than at the request boundary.
    let urls = [
        "s3://example-bucket/exports/report.csv",
        "s3://example-bucket/report",
        "s3://example-bucket/exports/nested/report.json",
    ];

    for row in rows() {
        for url in urls {
            let case = format!("{} with S3Url {url:?}", row.case);
            let reply = ses
                .create(json!({
                    "ExportDataSource": row.data_source,
                    "ExportDestination": { "DataFormat": row.data_format, "S3Url": url },
                }))
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected the input refusal, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case:?}: expected the operation's input error, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.message(),
                export_destination_s3_url_is_set_by_ses(),
                "case {case:?}: expected the modelled rule as the message, got {:?}",
                reply.message()
            );
        }
    }

    assert_eq!(
        ses.stored_count(),
        0,
        "a refused create stores nothing, got {} stored jobs",
        ses.stored_count()
    );
}
