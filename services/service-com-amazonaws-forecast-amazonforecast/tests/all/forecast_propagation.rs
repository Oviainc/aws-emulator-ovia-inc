// SPDX-License-Identifier: CC0-1.0

//! The forecast chain, end to end over the wire, across BOTH planes.
//!
//! The rule this suite is about: a resource the Amazon Forecast control plane mints is
//! readable through the service that reads that resource kind, and a resource it
//! deletes stops being readable there. For a forecast that reader is another service —
//! `forecastquery` (Amazon Forecast Runtime) `QueryForecast`, on the same shared engine
//! — so the suite mounts the control plane, the runtime and S3 on ONE engine and drives
//! the whole documented workflow: import training data, train a predictor, create a
//! forecast, query it, delete it.
//!
//! Every prediction asserted here is COMPUTED from the rows the import job read: the
//! expected values below are the documented estimator's own output
//! (`crate::state::series`) worked out by hand for each row of the table, never a value
//! copied out of a recording.
//!
//! The table spans what the rule generalises over: four forecast frequencies, both
//! documented timestamp formats and the default, quantile and `mean` forecast types,
//! one and two forecast dimensions, one and two time series, several rows inside one
//! bucket (summed) and separate ones, and `float` and `integer` targets.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_forecast_amazonforecast::EmulatedService;
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for: the credential scope is how the router
/// picks the service and where a minted ARN's region segment comes from.
const REGION: &str = "us-west-2";

/// The bucket the import files are uploaded to.
const BUCKET: &str = "forecast-training-data";

/// How long the clock owes a submitted import job, predictor or forecast before it
/// reaches `ACTIVE` ([`service_com_amazonaws_forecast_amazonforecast::state::PENDING_SPAN`]
/// plus `RUN_SPAN`), with room to spare on a loaded host.
const SETTLE: Duration = Duration::from_millis(1_200);

/// A dataset schema as `(AttributeName, AttributeType)` columns, in the order the
/// import file's columns carry them.
type Columns = &'static [(&'static str, &'static str)];

/// One import file row: the dimension values, the timestamp as written in the file, and
/// the target value.
#[derive(Debug, Clone, Copy)]
struct Row {
    dimensions: &'static [&'static str],
    timestamp: &'static str,
    value: &'static str,
}

/// One query a row of the table expects an answer to.
#[derive(Debug, Clone, Copy)]
struct Expectation {
    /// The `Filters` the query carries.
    filters: &'static [(&'static str, &'static str)],
    /// The statistic each forecast type is reported as, and the value the documented
    /// estimator gives for this series' history.
    statistics: &'static [(&'static str, f64)],
}

/// One row of the propagation table: a whole create → query → delete workflow.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// Names every resource of this row is created under.
    label: &'static str,
    domain: &'static str,
    columns: Columns,
    /// `TimestampFormat` the import names, or `None` to rely on the documented default
    /// (`yyyy-MM-dd HH:mm:ss`).
    timestamp_format: Option<&'static str>,
    frequency: &'static str,
    horizon: i64,
    forecast_types: &'static [&'static str],
    rows: &'static [Row],
    /// The timestamps every prediction of this row carries: the buckets that follow the
    /// last observed one, at the row's own frequency.
    timestamps: &'static [&'static str],
    expectations: &'static [Expectation],
}

/// `item_id, timestamp, demand` — the RETAIL target schema.
const RETAIL_TARGET: Columns = &[
    ("item_id", "string"),
    ("timestamp", "timestamp"),
    ("demand", "float"),
];

/// Two dimensions: `item_id` and `store_id`, as a forecast grouped by store has.
const STORE_TARGET: Columns = &[
    ("item_id", "string"),
    ("store_id", "string"),
    ("timestamp", "timestamp"),
    ("target_value", "float"),
];

/// An `integer` target, so the numeric column is not always a `float`.
const COUNT_TARGET: Columns = &[
    ("item_id", "string"),
    ("timestamp", "timestamp"),
    ("workforce_demand", "integer"),
];

/// Row A's history: eight daily observations, 10 to 80.
///
/// Sorted bucket values `[10,20,30,40,50,60,70,80]` (n = 8, last index 7), so the
/// documented estimator gives p10 = 10 + (20-10)*0.7 = 17, p50 = 40 + 10*0.5 = 45 and
/// p90 = 70 + 10*0.3 = 73.
const DAILY_ROWS: &[Row] = &[
    Row { dimensions: &["client_21"], timestamp: "2015-01-01", value: "10" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-02", value: "20" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-03", value: "30" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-04", value: "40" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-05", value: "50" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-06", value: "60" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-07", value: "70" },
    Row { dimensions: &["client_21"], timestamp: "2015-01-08", value: "80" },
];

/// Row B's history: two series of eight hourly observations each, written in the
/// default timestamp format. Means are 18 and 4.5.
const HOURLY_ROWS: &[Row] = &[
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 00:00:00", value: "4" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 01:00:00", value: "8" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 02:00:00", value: "12" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 03:00:00", value: "16" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 04:00:00", value: "20" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 05:00:00", value: "24" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 06:00:00", value: "28" },
    Row { dimensions: &["item_1", "store_1"], timestamp: "2020-03-01 07:00:00", value: "32" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 00:00:00", value: "1" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 01:00:00", value: "2" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 02:00:00", value: "3" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 03:00:00", value: "4" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 04:00:00", value: "5" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 05:00:00", value: "6" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 06:00:00", value: "7" },
    Row { dimensions: &["item_1", "store_2"], timestamp: "2020-03-01 07:00:00", value: "8" },
];

/// Row C's history: eight weekly buckets, each written as TWO rows (a Monday and the
/// Wednesday after it) whose values the documented default aggregation sums.
///
/// Bucket values `[10,20,30,40,50,60,70,100]`, so p50 = 45 and the mean is 47.5 — which
/// differ, so a mean reported as a median would fail.
const WEEKLY_ROWS: &[Row] = &[
    Row { dimensions: &["sku_7"], timestamp: "2021-01-04", value: "5" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-06", value: "5" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-11", value: "10" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-13", value: "10" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-18", value: "15" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-20", value: "15" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-25", value: "20" },
    Row { dimensions: &["sku_7"], timestamp: "2021-01-27", value: "20" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-01", value: "25" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-03", value: "25" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-08", value: "30" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-10", value: "30" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-15", value: "35" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-17", value: "35" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-22", value: "50" },
    Row { dimensions: &["sku_7"], timestamp: "2021-02-24", value: "50" },
];

/// Row D's history: twelve monthly observations, 1 to 12, written mid-month so the
/// monthly bucket's own boundary (the 1st) is what the predictions sit on.
///
/// n = 12 (last index 11), so p1 = 1 + 0.11 = 1.11 and p99 = 11 + 0.89 = 11.89.
const MONTHLY_ROWS: &[Row] = &[
    Row { dimensions: &["crew_a"], timestamp: "2019-01-15", value: "1" },
    Row { dimensions: &["crew_a"], timestamp: "2019-02-15", value: "2" },
    Row { dimensions: &["crew_a"], timestamp: "2019-03-15", value: "3" },
    Row { dimensions: &["crew_a"], timestamp: "2019-04-15", value: "4" },
    Row { dimensions: &["crew_a"], timestamp: "2019-05-15", value: "5" },
    Row { dimensions: &["crew_a"], timestamp: "2019-06-15", value: "6" },
    Row { dimensions: &["crew_a"], timestamp: "2019-07-15", value: "7" },
    Row { dimensions: &["crew_a"], timestamp: "2019-08-15", value: "8" },
    Row { dimensions: &["crew_a"], timestamp: "2019-09-15", value: "9" },
    Row { dimensions: &["crew_a"], timestamp: "2019-10-15", value: "10" },
    Row { dimensions: &["crew_a"], timestamp: "2019-11-15", value: "11" },
    Row { dimensions: &["crew_a"], timestamp: "2019-12-15", value: "12" },
];

/// The table. Each row is one whole workflow; the issue's own case (a daily forecast of
/// one item queried by `item_id`) is just the first.
const CASES: &[Case] = &[
    Case {
        label: "daily_quantiles",
        domain: "RETAIL",
        columns: RETAIL_TARGET,
        timestamp_format: Some("yyyy-MM-dd"),
        frequency: "D",
        horizon: 2,
        forecast_types: &["0.1", "0.5", "0.9"],
        rows: DAILY_ROWS,
        timestamps: &["2015-01-09T00:00:00", "2015-01-10T00:00:00"],
        expectations: &[Expectation {
            filters: &[("item_id", "client_21")],
            statistics: &[("p10", 17.0), ("p50", 45.0), ("p90", 73.0)],
        }],
    },
    Case {
        label: "hourly_mean_by_store",
        domain: "CUSTOM",
        columns: STORE_TARGET,
        // No TimestampFormat: the documented default is `yyyy-MM-dd HH:mm:ss`.
        timestamp_format: None,
        frequency: "H",
        horizon: 2,
        forecast_types: &["mean"],
        rows: HOURLY_ROWS,
        timestamps: &["2020-03-01T08:00:00", "2020-03-01T09:00:00"],
        expectations: &[
            Expectation {
                filters: &[("item_id", "item_1"), ("store_id", "store_1")],
                statistics: &[("mean", 18.0)],
            },
            Expectation {
                filters: &[("item_id", "item_1"), ("store_id", "store_2")],
                statistics: &[("mean", 4.5)],
            },
        ],
    },
    Case {
        label: "weekly_median_and_mean",
        domain: "INVENTORY_PLANNING",
        columns: RETAIL_TARGET,
        timestamp_format: Some("yyyy-MM-dd"),
        frequency: "W",
        horizon: 2,
        forecast_types: &["0.5", "mean"],
        rows: WEEKLY_ROWS,
        timestamps: &["2021-03-01T00:00:00", "2021-03-08T00:00:00"],
        expectations: &[Expectation {
            filters: &[("item_id", "sku_7")],
            statistics: &[("p50", 45.0), ("mean", 47.5)],
        }],
    },
    Case {
        label: "monthly_extreme_quantiles",
        domain: "WORK_FORCE",
        columns: COUNT_TARGET,
        timestamp_format: Some("yyyy-MM-dd"),
        frequency: "M",
        horizon: 3,
        forecast_types: &["0.01", "0.99"],
        rows: MONTHLY_ROWS,
        timestamps: &[
            "2020-01-01T00:00:00",
            "2020-02-01T00:00:00",
            "2020-03-01T00:00:00",
        ],
        expectations: &[Expectation {
            filters: &[("item_id", "crew_a")],
            statistics: &[("p1", 1.11), ("p99", 11.89)],
        }],
    },
];

/// One emulator: the Forecast control plane, the Amazon Forecast Runtime data plane and
/// S3, all three on ONE shared engine — which is what the assembled router does.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are verified by the emulator, never by a service; this suite is
        // about behaviour, so it signs with a fixed scope and verifies nothing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One `awsJson1_1` call to a service routed by `target` prefix.
    async fn target_call(&self, target: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/forecast/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A control-plane call (`AmazonForecast.<Operation>`).
    async fn control(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.target_call("AmazonForecast", operation, body).await
    }

    /// A data-plane call (`AmazonForecastRuntime.<Operation>`).
    async fn runtime(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.target_call("AmazonForecastRuntime", operation, body)
            .await
    }

    /// A control-plane call that must succeed.
    async fn control_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.control(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One path-style request to the emulator's S3 front door: S3 is a REST service, so
    /// the bucket and key live in the URI and the body is the object's bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        let request = ::http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Create [`BUCKET`] and upload one import file to it.
    ///
    /// The create names its `LocationConstraint`: a create that names none can only mean
    /// us-east-1, the Region the unspecified constraint stands for, so S3 refuses one sent
    /// to any other Region's endpoint and this harness signs for [`REGION`].
    async fn upload(&self, key: &str, body: &str) {
        let created = self
            .s3(
                "PUT",
                &format!("/{BUCKET}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert!(
            created == 200 || created == 409,
            "CreateBucket {BUCKET}: expected 200 or 409, got {created}"
        );
        let put = self.s3("PUT", &format!("/{BUCKET}/{key}"), body.as_bytes()).await;
        assert_eq!(put, 200, "PutObject {key}: expected 200, got {put}");
    }
}

/// The wire error code of a failure response, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of a failure response.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `Forecast.Predictions` as `statistic -> [(timestamp, value)]`.
fn predictions(body: &Value) -> BTreeMap<String, Vec<(String, f64)>> {
    body.get("Forecast")
        .and_then(|forecast| forecast.get("Predictions"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(statistic, points)| {
                    let points = points
                        .as_array()
                        .expect("Predictions value must be a list")
                        .iter()
                        .map(|point| {
                            (
                                point["Timestamp"].as_str().unwrap_or_default().to_string(),
                                point["Value"].as_f64().unwrap_or_default(),
                            )
                        })
                        .collect();
                    (statistic.clone(), points)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN this emulator must mint for a Forecast resource of `resource`.
fn arn(resource: &str) -> String {
    format!(
        "arn:aws:forecast:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

fn schema_json(columns: Columns) -> Value {
    json!({
        "Attributes": columns
            .iter()
            .map(|(name, kind)| json!({"AttributeName": name, "AttributeType": kind}))
            .collect::<Vec<_>>(),
    })
}

/// The import file a row of the table uploads: Forecast's CSV format, which carries no
/// header row.
fn import_file(case: &Case) -> String {
    let timestamp_column = case
        .columns
        .iter()
        .position(|(_, kind)| *kind == "timestamp")
        .expect("the schema declares a timestamp column");
    let mut text = String::new();
    for row in case.rows {
        let mut fields: Vec<&str> = Vec::with_capacity(case.columns.len());
        fields.extend(row.dimensions.iter().copied());
        fields.insert(timestamp_column, row.timestamp);
        fields.push(row.value);
        text.push_str(&fields.join(","));
        text.push('\n');
    }
    text
}

/// Whether two prediction values agree to within floating-point noise.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-6
}

/// Wait out the span the clock owes a submitted run before it reaches `ACTIVE`.
async fn settle() {
    ::tokio::time::sleep(SETTLE).await;
}

/// Drive one row of the table from `CreateDataset` to an `ACTIVE` forecast, returning
/// the forecast's ARN.
async fn create_forecast_for(emulator: &Emulator, case: &Case) -> String {
    let label = case.label;
    let dataset = emulator
        .control_ok(
            "CreateDataset",
            &json!({
                "DatasetName": format!("{label}_data"),
                "Domain": case.domain,
                "DatasetType": "TARGET_TIME_SERIES",
                "DataFrequency": case.frequency,
                "Schema": schema_json(case.columns),
            }),
        )
        .await;
    let dataset_arn = dataset["DatasetArn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        dataset_arn,
        arn(&format!("dataset/{label}_data")),
        "case {label}: unexpected DatasetArn"
    );

    let group = emulator
        .control_ok(
            "CreateDatasetGroup",
            &json!({
                "DatasetGroupName": format!("{label}_group"),
                "Domain": case.domain,
                "DatasetArns": [dataset_arn],
            }),
        )
        .await;
    let group_arn = group["DatasetGroupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let key = format!("{label}/history.csv");
    emulator.upload(&key, &import_file(case)).await;

    let mut import = json!({
        "DatasetImportJobName": format!("{label}_import"),
        "DatasetArn": dataset_arn,
        "DataSource": {"S3Config": {
            "Path": format!("s3://{BUCKET}/{key}"),
            "RoleArn": arn("role/forecast_access"),
        }},
    });
    if let Some(format) = case.timestamp_format {
        import["TimestampFormat"] = json!(format);
    }
    let job = emulator.control_ok("CreateDatasetImportJob", &import).await;
    let job_arn = job["DatasetImportJobArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        job_arn,
        arn(&format!(
            "dataset-import-job/{label}_data/{label}_import"
        )),
        "case {label}: unexpected DatasetImportJobArn"
    );

    settle().await;
    // The round trip: the import job reads back through its own Describe, ACTIVE and
    // carrying the statistics of the rows that were read.
    let described = emulator
        .control_ok(
            "DescribeDatasetImportJob",
            &json!({"DatasetImportJobArn": job_arn}),
        )
        .await;
    assert_eq!(
        described["Status"].as_str(),
        Some("ACTIVE"),
        "case {label}: expected an ACTIVE import job, got {described}"
    );
    let counted = described["FieldStatistics"]
        .get(case.columns[0].0)
        .and_then(|statistics| statistics["Count"].as_i64());
    assert_eq!(
        counted,
        Some(case.rows.len() as i64),
        "case {label}: expected the import's statistics to count {} rows, got {described}",
        case.rows.len()
    );

    let predictor = emulator
        .control_ok(
            "CreateAutoPredictor",
            &json!({
                "PredictorName": format!("{label}_predictor"),
                "ForecastHorizon": case.horizon,
                "ForecastFrequency": case.frequency,
                "ForecastTypes": case.forecast_types,
                "DataConfig": {"DatasetGroupArn": group_arn},
            }),
        )
        .await;
    let predictor_arn = predictor["PredictorArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    settle().await;
    let described = emulator
        .control_ok(
            "DescribeAutoPredictor",
            &json!({"PredictorArn": predictor_arn}),
        )
        .await;
    assert_eq!(
        described["Status"].as_str(),
        Some("ACTIVE"),
        "case {label}: expected an ACTIVE predictor, got {described}"
    );
    assert_eq!(
        described["DatasetImportJobArns"],
        json!([job_arn]),
        "case {label}: expected the predictor to name the import job it trained on, got \
         {described}"
    );

    let forecast = emulator
        .control_ok(
            "CreateForecast",
            &json!({
                "ForecastName": format!("{label}_forecast"),
                "PredictorArn": predictor_arn,
            }),
        )
        .await;
    let forecast_arn = forecast["ForecastArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        forecast_arn,
        arn(&format!("forecast/{label}_forecast")),
        "case {label}: unexpected ForecastArn"
    );

    settle().await;
    let described = emulator
        .control_ok("DescribeForecast", &json!({"ForecastArn": forecast_arn}))
        .await;
    assert_eq!(
        described["Status"].as_str(),
        Some("ACTIVE"),
        "case {label}: expected an ACTIVE forecast, got {described}"
    );
    assert_eq!(
        described["PredictorArn"].as_str(),
        Some(predictor_arn.as_str()),
        "case {label}: expected the forecast to name its predictor, got {described}"
    );
    assert_eq!(
        described["ForecastTypes"],
        json!(case.forecast_types),
        "case {label}: expected the forecast to report the types it was created with, \
         got {described}"
    );

    forecast_arn
}

// ---------------------------------------------------------------------------
// The rule: a forecast the control plane created is queryable through the runtime
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_forecast_is_queryable_and_a_deleted_one_is_not() {
    for case in CASES {
        let label = case.label;
        let emulator = Emulator::new();
        let forecast_arn = create_forecast_for(&emulator, case).await;

        for expectation in case.expectations {
            let filters: BTreeMap<&str, &str> = expectation.filters.iter().copied().collect();
            let (status, body) = emulator
                .runtime(
                    "QueryForecast",
                    &json!({"ForecastArn": forecast_arn, "Filters": filters}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label} {filters:?}: expected QueryForecast 200 for a forecast the \
                 control plane created, got {status} with {body}"
            );
            let answered = predictions(&body);
            let expected_statistics: Vec<String> = expectation
                .statistics
                .iter()
                .map(|(statistic, _)| (*statistic).to_string())
                .collect();
            let mut answered_statistics: Vec<String> = answered.keys().cloned().collect();
            answered_statistics.sort();
            let mut wanted = expected_statistics.clone();
            wanted.sort();
            assert_eq!(
                answered_statistics, wanted,
                "case {label} {filters:?}: expected the statistics of the forecast types \
                 it was created with, got {answered:?}"
            );
            for (statistic, expected) in expectation.statistics {
                let points = answered
                    .get(*statistic)
                    .unwrap_or_else(|| panic!("case {label}: no {statistic} in {answered:?}"));
                let timestamps: Vec<&str> =
                    points.iter().map(|(at, _)| at.as_str()).collect();
                assert_eq!(
                    timestamps, case.timestamps,
                    "case {label} {filters:?} {statistic}: expected the horizon's own \
                     buckets, got {timestamps:?}"
                );
                for (at, value) in points {
                    assert!(
                        close(*value, *expected),
                        "case {label} {filters:?} {statistic} at {at}: expected \
                         {expected}, got {value}"
                    );
                }
            }
        }

        // The other half of the rule: a deleted forecast stops being queryable, and the
        // control plane stops describing it.
        let (status, body) = emulator
            .control("DeleteForecast", &json!({"ForecastArn": forecast_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: expected DeleteForecast 200, got {status} with {body}"
        );
        let filters: BTreeMap<&str, &str> = case.expectations[0].filters.iter().copied().collect();
        let (status, body) = emulator
            .runtime(
                "QueryForecast",
                &json!({"ForecastArn": forecast_arn, "Filters": filters}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {label}: expected QueryForecast 404 after the delete, got {status} with \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException after the delete, got {body}"
        );
        let (status, body) = emulator
            .control("DescribeForecast", &json!({"ForecastArn": forecast_arn}))
            .await;
        assert_eq!(
            status, 404,
            "case {label}: expected DescribeForecast 404 after the delete, got {status} \
             with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The transient state the clock owes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_forecast_is_not_queryable_before_its_creation_run_is_active() {
    let case = &CASES[0];
    let emulator = Emulator::new();
    let label = case.label;

    // The same chain, but stopping at the instant `CreateForecast` answered.
    let dataset = emulator
        .control_ok(
            "CreateDataset",
            &json!({
                "DatasetName": format!("{label}_pending"),
                "Domain": case.domain,
                "DatasetType": "TARGET_TIME_SERIES",
                "DataFrequency": case.frequency,
                "Schema": schema_json(case.columns),
            }),
        )
        .await;
    let dataset_arn = dataset["DatasetArn"].as_str().unwrap_or_default().to_string();
    let group = emulator
        .control_ok(
            "CreateDatasetGroup",
            &json!({
                "DatasetGroupName": format!("{label}_pending_group"),
                "Domain": case.domain,
                "DatasetArns": [dataset_arn],
            }),
        )
        .await;
    let group_arn = group["DatasetGroupArn"].as_str().unwrap_or_default().to_string();
    let key = "pending/history.csv";
    emulator.upload(key, &import_file(case)).await;
    let job = emulator
        .control_ok(
            "CreateDatasetImportJob",
            &json!({
                "DatasetImportJobName": "pending_import",
                "DatasetArn": dataset_arn,
                "TimestampFormat": case.timestamp_format,
                "DataSource": {"S3Config": {
                    "Path": format!("s3://{BUCKET}/{key}"),
                    "RoleArn": arn("role/forecast_access"),
                }},
            }),
        )
        .await;
    let job_arn = job["DatasetImportJobArn"].as_str().unwrap_or_default().to_string();

    // A predictor cannot be trained on an import the clock has not finished.
    let (status, body) = emulator
        .control(
            "CreateAutoPredictor",
            &json!({
                "PredictorName": "pending_predictor_too_early",
                "ForecastHorizon": case.horizon,
                "ForecastFrequency": case.frequency,
                "DataConfig": {"DatasetGroupArn": group_arn},
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "expected a predictor trained on an unfinished import to be refused 409, got \
         {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "expected ResourceInUseException for an unfinished import, got {body}"
    );
    let described = emulator
        .control_ok(
            "DescribeDatasetImportJob",
            &json!({"DatasetImportJobArn": job_arn}),
        )
        .await;
    assert!(
        matches!(
            described["Status"].as_str(),
            Some("CREATE_PENDING") | Some("CREATE_IN_PROGRESS")
        ),
        "expected the import job to still be being created, got {described}"
    );

    settle().await;
    let predictor = emulator
        .control_ok(
            "CreateAutoPredictor",
            &json!({
                "PredictorName": "pending_predictor",
                "ForecastHorizon": case.horizon,
                "ForecastFrequency": case.frequency,
                "DataConfig": {"DatasetGroupArn": group_arn},
            }),
        )
        .await;
    let predictor_arn = predictor["PredictorArn"].as_str().unwrap_or_default().to_string();

    // A forecast cannot be created from a predictor that is still training.
    let (status, body) = emulator
        .control(
            "CreateForecast",
            &json!({
                "ForecastName": "pending_forecast_too_early",
                "PredictorArn": predictor_arn,
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "expected a forecast from a predictor still training to be refused 409, got \
         {status} with {body}"
    );

    settle().await;
    let forecast = emulator
        .control_ok(
            "CreateForecast",
            &json!({
                "ForecastName": "pending_forecast",
                "PredictorArn": predictor_arn,
            }),
        )
        .await;
    let forecast_arn = forecast["ForecastArn"].as_str().unwrap_or_default().to_string();

    // The forecast exists in the runtime store, and is NOT readable yet: "The Status of
    // the forecast must be ACTIVE before you can query or export the forecast".
    let filters: BTreeMap<&str, &str> = case.expectations[0].filters.iter().copied().collect();
    let (status, body) = emulator
        .runtime(
            "QueryForecast",
            &json!({"ForecastArn": forecast_arn, "Filters": filters}),
        )
        .await;
    assert_eq!(
        status, 409,
        "expected QueryForecast 409 while the forecast is still being created, got \
         {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "expected ResourceInUseException while the forecast is being created, got {body}"
    );
    assert!(
        message(&body).contains("ACTIVE"),
        "expected the refusal to say the status must be ACTIVE, got {body}"
    );
    // And a forecast still being created cannot be deleted.
    let (status, body) = emulator
        .control("DeleteForecast", &json!({"ForecastArn": forecast_arn}))
        .await;
    assert_eq!(
        status, 409,
        "expected DeleteForecast 409 while the forecast is still being created, got \
         {status} with {body}"
    );

    settle().await;
    let (status, body) = emulator
        .runtime(
            "QueryForecast",
            &json!({"ForecastArn": forecast_arn, "Filters": filters}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected QueryForecast 200 once the forecast is ACTIVE, got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// The same rule for a what-if forecast
// ---------------------------------------------------------------------------

/// One what-if case: the transformations it applies to the baseline, and the statistics
/// the transformed history's own quantiles give.
///
/// The baseline is [`CASES`]`[0]`: bucket values `[10,20,30,40,50,60,70,80]`, so p10 =
/// 17, p50 = 45 and p90 = 73. Each row below is those values transformed and re-fitted,
/// worked out by hand.
#[derive(Debug, Clone, Copy)]
struct WhatIfCase {
    name: &'static str,
    /// `(AttributeName, Operation, Value)` actions, applied in order.
    actions: &'static [(&'static str, &'static str, f64)],
    /// `(AttributeName, AttributeValue, Condition)` conditions on every action.
    conditions: &'static [(&'static str, &'static str, &'static str)],
    statistics: &'static [(&'static str, f64)],
}

/// The what-if table: each arithmetic operation the model declares, a condition that
/// selects the series, a condition that does not, an attribute the estimator does not
/// read, and two actions in sequence.
const WHAT_IF_CASES: &[WhatIfCase] = &[
    WhatIfCase {
        name: "demand_doubled",
        actions: &[("demand", "MULTIPLY", 2.0)],
        conditions: &[],
        // [20,40,…,160]
        statistics: &[("p10", 34.0), ("p50", 90.0), ("p90", 146.0)],
    },
    WhatIfCase {
        name: "demand_plus_five",
        actions: &[("demand", "ADD", 5.0)],
        conditions: &[],
        statistics: &[("p10", 22.0), ("p50", 50.0), ("p90", 78.0)],
    },
    WhatIfCase {
        name: "demand_minus_ten",
        actions: &[("demand", "SUBTRACT", 10.0)],
        conditions: &[],
        statistics: &[("p10", 7.0), ("p50", 35.0), ("p90", 63.0)],
    },
    WhatIfCase {
        name: "demand_halved",
        actions: &[("demand", "DIVIDE", 2.0)],
        conditions: &[],
        statistics: &[("p10", 8.5), ("p50", 22.5), ("p90", 36.5)],
    },
    WhatIfCase {
        name: "demand_tripled_for_this_item",
        actions: &[("demand", "MULTIPLY", 3.0)],
        conditions: &[("item_id", "client_21", "EQUALS")],
        // [30,60,…,240]
        statistics: &[("p10", 51.0), ("p50", 135.0), ("p90", 219.0)],
    },
    WhatIfCase {
        name: "demand_tripled_for_another_item",
        actions: &[("demand", "MULTIPLY", 3.0)],
        // The condition does not hold for this series, so "an action is applied only
        // when all conditions are met" leaves it as the baseline has it.
        conditions: &[("item_id", "client_99", "EQUALS")],
        statistics: &[("p10", 17.0), ("p50", 45.0), ("p90", 73.0)],
    },
    WhatIfCase {
        name: "price_doubled",
        // An attribute this emulator's estimator does not read: the baseline's own
        // series, unchanged, rather than an invented effect.
        actions: &[("price", "MULTIPLY", 2.0)],
        conditions: &[],
        statistics: &[("p10", 17.0), ("p50", 45.0), ("p90", 73.0)],
    },
    WhatIfCase {
        name: "demand_plus_five_then_doubled",
        actions: &[("demand", "ADD", 5.0), ("demand", "MULTIPLY", 2.0)],
        conditions: &[],
        // [30,50,…,170]
        statistics: &[("p10", 44.0), ("p50", 100.0), ("p90", 156.0)],
    },
];

/// The `TimeSeriesTransformations` a what-if case submits.
fn transformations_json(case: &WhatIfCase) -> Value {
    json!(case
        .actions
        .iter()
        .map(|(attribute, operation, value)| {
            let mut transformation = json!({
                "Action": {
                    "AttributeName": attribute,
                    "Operation": operation,
                    "Value": value,
                },
            });
            if !case.conditions.is_empty() {
                transformation["TimeSeriesConditions"] = json!(case
                    .conditions
                    .iter()
                    .map(|(attribute, value, condition)| json!({
                        "AttributeName": attribute,
                        "AttributeValue": value,
                        "Condition": condition,
                    }))
                    .collect::<Vec<_>>());
            }
            transformation
        })
        .collect::<Vec<_>>())
}

#[tokio::test]
async fn a_created_what_if_forecast_is_queryable_and_a_deleted_one_is_not() {
    let case = &CASES[0];
    let emulator = Emulator::new();
    let forecast_arn = create_forecast_for(&emulator, case).await;

    let analysis = emulator
        .control_ok(
            "CreateWhatIfAnalysis",
            &json!({
                "WhatIfAnalysisName": "price_cut",
                "ForecastArn": forecast_arn,
            }),
        )
        .await;
    let analysis_arn = analysis["WhatIfAnalysisArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        analysis_arn,
        arn("what-if-analysis/price_cut"),
        "unexpected WhatIfAnalysisArn"
    );

    settle().await;
    let described = emulator
        .control_ok(
            "DescribeWhatIfAnalysis",
            &json!({"WhatIfAnalysisArn": analysis_arn}),
        )
        .await;
    assert_eq!(
        described["Status"].as_str(),
        Some("ACTIVE"),
        "expected an ACTIVE what-if analysis, got {described}"
    );
    assert_eq!(
        described["ForecastArn"].as_str(),
        Some(forecast_arn.as_str()),
        "expected the analysis to name its baseline forecast, got {described}"
    );

    // Every row of the table, created in one analysis: a what-if forecast's name is
    // "unique within each what-if analysis", which is why its ARN carries both names.
    let mut created: Vec<(&WhatIfCase, String)> = Vec::new();
    for what_if in WHAT_IF_CASES {
        let answer = emulator
            .control_ok(
                "CreateWhatIfForecast",
                &json!({
                    "WhatIfForecastName": what_if.name,
                    "WhatIfAnalysisArn": analysis_arn,
                    "TimeSeriesTransformations": transformations_json(what_if),
                }),
            )
            .await;
        let what_if_arn = answer["WhatIfForecastArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            what_if_arn,
            arn(&format!("what-if-forecast/price_cut/{}", what_if.name)),
            "case {}: unexpected WhatIfForecastArn",
            what_if.name
        );
        created.push((what_if, what_if_arn));
    }

    settle().await;
    let filters: BTreeMap<&str, &str> = case.expectations[0].filters.iter().copied().collect();
    for (what_if, what_if_arn) in &created {
        let label = what_if.name;
        let described = emulator
            .control_ok(
                "DescribeWhatIfForecast",
                &json!({"WhatIfForecastArn": what_if_arn}),
            )
            .await;
        assert_eq!(
            described["Status"].as_str(),
            Some("ACTIVE"),
            "case {label}: expected an ACTIVE what-if forecast, got {described}"
        );
        assert_eq!(
            described["WhatIfAnalysisArn"].as_str(),
            Some(analysis_arn.as_str()),
            "case {label}: expected the what-if forecast to name its analysis, got \
             {described}"
        );
        assert_eq!(
            described["ForecastTypes"],
            json!(case.forecast_types),
            "case {label}: expected the baseline's forecast types, got {described}"
        );

        let (status, body) = emulator
            .runtime(
                "QueryWhatIfForecast",
                &json!({"WhatIfForecastArn": what_if_arn, "Filters": filters}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label}: expected QueryWhatIfForecast 200 for a what-if forecast the \
             control plane created, got {status} with {body}"
        );
        let answered = predictions(&body);
        for (statistic, expected) in what_if.statistics {
            let points = answered
                .get(*statistic)
                .unwrap_or_else(|| panic!("case {label}: no {statistic} in {answered:?}"));
            let timestamps: Vec<&str> = points.iter().map(|(at, _)| at.as_str()).collect();
            assert_eq!(
                timestamps, case.timestamps,
                "case {label} {statistic}: expected the baseline's own horizon, got \
                 {timestamps:?}"
            );
            for (at, value) in points {
                assert!(
                    close(*value, *expected),
                    "case {label} {statistic} at {at}: expected {expected}, got {value}"
                );
            }
        }
    }

    // A what-if forecast the baseline's own query must not answer, and the other way
    // round: the two ARNs name different resources in different stores.
    let (status, body) = emulator
        .runtime(
            "QueryForecast",
            &json!({"ForecastArn": created[0].1, "Filters": filters}),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected QueryForecast to refuse a what-if forecast ARN, got {status} with {body}"
    );

    // Deleting one what-if forecast withdraws exactly it.
    let (what_if, deleted_arn) = &created[0];
    let (status, body) = emulator
        .control(
            "DeleteWhatIfForecast",
            &json!({"WhatIfForecastArn": deleted_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {}: expected DeleteWhatIfForecast 200, got {status} with {body}",
        what_if.name
    );
    let (status, body) = emulator
        .runtime(
            "QueryWhatIfForecast",
            &json!({"WhatIfForecastArn": deleted_arn, "Filters": filters}),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected QueryWhatIfForecast 404 after the delete, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException after the delete, got {body}"
    );
    let (status, body) = emulator
        .runtime(
            "QueryWhatIfForecast",
            &json!({"WhatIfForecastArn": created[1].1, "Filters": filters}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the other what-if forecasts to keep answering, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_what_if_forecast_this_emulator_cannot_compute_is_refused() {
    let case = &CASES[0];
    let emulator = Emulator::new();
    let forecast_arn = create_forecast_for(&emulator, case).await;

    // An analysis of a forecast nothing created.
    let (status, body) = emulator
        .control(
            "CreateWhatIfAnalysis",
            &json!({
                "WhatIfAnalysisName": "no_baseline",
                "ForecastArn": arn("forecast/never_created"),
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected an analysis of an unknown forecast to be refused 404, got {status} with \
         {body}"
    );

    // A what-if forecast in an analysis nothing created.
    let (status, body) = emulator
        .control(
            "CreateWhatIfForecast",
            &json!({
                "WhatIfForecastName": "no_analysis",
                "WhatIfAnalysisArn": arn("what-if-analysis/never_created"),
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected a what-if forecast in an unknown analysis to be refused 404, got \
         {status} with {body}"
    );

    let analysis = emulator
        .control_ok(
            "CreateWhatIfAnalysis",
            &json!({"WhatIfAnalysisName": "refusals", "ForecastArn": forecast_arn}),
        )
        .await;
    let analysis_arn = analysis["WhatIfAnalysisArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    settle().await;

    for (label, body, expected) in [
        (
            "a replacement related time series, which the estimator does not read",
            json!({
                "WhatIfForecastName": "replacements",
                "WhatIfAnalysisArn": analysis_arn,
                "TimeSeriesReplacementsDataSource": {
                    "S3Config": {
                        "Path": format!("s3://{BUCKET}/replacements/"),
                        "RoleArn": arn("role/forecast_access"),
                    },
                    "Schema": schema_json(RETAIL_TARGET),
                },
            }),
            "TimeSeriesReplacementsDataSource",
        ),
        (
            "a DIVIDE transformation by zero",
            json!({
                "WhatIfForecastName": "divide_by_zero",
                "WhatIfAnalysisArn": analysis_arn,
                "TimeSeriesTransformations": [{
                    "Action": {"AttributeName": "demand", "Operation": "DIVIDE", "Value": 0.0},
                }],
            }),
            "divide by zero",
        ),
    ] {
        let (status, answer) = emulator.control("CreateWhatIfForecast", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidInputException",
            "case {label:?}: expected InvalidInputException, got {answer}"
        );
        assert!(
            message(&answer).contains(expected),
            "case {label:?}: expected the refusal to mention {expected:?}, got {answer}"
        );
    }
}

/// One refusal case: a request the chain cannot carry out, and the answer it earns.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    label: &'static str,
    operation: &'static str,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn the_chain_refuses_what_it_cannot_compute() {
    let case = &CASES[0];
    let emulator = Emulator::new();
    let dataset = emulator
        .control_ok(
            "CreateDataset",
            &json!({
                "DatasetName": "refusals_data",
                "Domain": case.domain,
                "DatasetType": "TARGET_TIME_SERIES",
                "DataFrequency": case.frequency,
                "Schema": schema_json(case.columns),
            }),
        )
        .await;
    let dataset_arn = dataset["DatasetArn"].as_str().unwrap_or_default().to_string();
    let group = emulator
        .control_ok(
            "CreateDatasetGroup",
            &json!({
                "DatasetGroupName": "refusals_group",
                "Domain": case.domain,
                "DatasetArns": [dataset_arn],
            }),
        )
        .await;
    let group_arn = group["DatasetGroupArn"].as_str().unwrap_or_default().to_string();

    // A predictor over a dataset group nothing has been imported into: there is no
    // history to fit, and a model fitted to nothing would be a fabricated predictor.
    let (status, body) = emulator
        .control(
            "CreateAutoPredictor",
            &json!({
                "PredictorName": "no_data_predictor",
                "ForecastHorizon": 2,
                "ForecastFrequency": "D",
                "DataConfig": {"DatasetGroupArn": group_arn},
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a predictor over an empty dataset group to be refused 400, got {status} \
         with {body}"
    );
    assert!(
        message(&body).contains("No data has been imported"),
        "expected the refusal to name the missing import, got {body}"
    );

    // A forecast from a predictor that does not exist.
    let (status, body) = emulator
        .control(
            "CreateForecast",
            &json!({
                "ForecastName": "no_predictor_forecast",
                "PredictorArn": arn("predictor/not_there"),
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected a forecast from an unknown predictor to be refused 404, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for an unknown predictor, got {body}"
    );

    // A query for a forecast nothing created: the store's answer, unchanged by any of
    // the above.
    let (status, body) = emulator
        .runtime(
            "QueryForecast",
            &json!({
                "ForecastArn": arn("forecast/never_created"),
                "Filters": {"item_id": "client_21"},
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected QueryForecast 404 for a forecast nothing created, got {status} with \
         {body}"
    );

    // An import this emulator cannot read is refused at the submission (PARQUET) or
    // recorded as a failed run (every other case below) — never answered as a success
    // that imported nothing.
    emulator
        .upload("refusals/history.csv", &import_file(case))
        .await;
    emulator
        .upload("refusals/short.csv", "client_21,2015-01-01\n")
        .await;

    let (status, body) = emulator
        .control(
            "CreateDatasetImportJob",
            &json!({
                "DatasetImportJobName": "parquet_import",
                "DatasetArn": dataset_arn,
                "Format": "PARQUET",
                "DataSource": {"S3Config": {
                    "Path": format!("s3://{BUCKET}/refusals/history.csv"),
                    "RoleArn": arn("role/forecast_access"),
                }},
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a PARQUET import to be refused 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "expected InvalidInputException for a PARQUET import, got {body}"
    );

    for (label, name, path, expected) in [
        (
            "a row with fewer fields than the schema declares",
            "short_import",
            format!("s3://{BUCKET}/refusals/short.csv"),
            "fields",
        ),
        (
            "a path with no import file under it",
            "missing_import",
            format!("s3://{BUCKET}/refusals/not-there/"),
            "No import files were found",
        ),
    ] {
        let job = emulator
            .control_ok(
                "CreateDatasetImportJob",
                &json!({
                    "DatasetImportJobName": name,
                    "DatasetArn": dataset_arn,
                    "TimestampFormat": "yyyy-MM-dd",
                    "DataSource": {"S3Config": {
                        "Path": path,
                        "RoleArn": arn("role/forecast_access"),
                    }},
                }),
            )
            .await;
        let job_arn = job["DatasetImportJobArn"].as_str().unwrap_or_default().to_string();
        settle().await;
        let described = emulator
            .control_ok(
                "DescribeDatasetImportJob",
                &json!({"DatasetImportJobArn": job_arn}),
            )
            .await;
        assert_eq!(
            described["Status"].as_str(),
            Some("CREATE_FAILED"),
            "case {label:?}: expected a CREATE_FAILED import job, got {described}"
        );
        let reported = described["Message"].as_str().unwrap_or_default();
        assert!(
            reported.contains(expected),
            "case {label:?}: expected the failure to mention {expected:?}, got {reported:?}"
        );
    }

    // A horizon longer than the data supports: "the lesser of 500 time-steps or 1/4 of
    // the TARGET_TIME_SERIES dataset length" — the history above holds 8 time-steps, so
    // 2 is the most a new predictor may name.
    emulator
        .control_ok(
            "CreateDatasetImportJob",
            &json!({
                "DatasetImportJobName": "good_import",
                "DatasetArn": dataset_arn,
                "TimestampFormat": "yyyy-MM-dd",
                "DataSource": {"S3Config": {
                    "Path": format!("s3://{BUCKET}/refusals/history.csv"),
                    "RoleArn": arn("role/forecast_access"),
                }},
            }),
        )
        .await;
    settle().await;

    for refusal in [
        Refusal {
            label: "a horizon past 1/4 of the training history",
            operation: "CreateAutoPredictor",
            status: 400,
            code: "InvalidInputException",
        },
        Refusal {
            label: "a horizon past the published maximum of 500",
            operation: "CreateAutoPredictor",
            status: 400,
            code: "InvalidInputException",
        },
    ] {
        let horizon = if refusal.label.contains("500") { 501 } else { 3 };
        let (status, body) = emulator
            .control(
                refusal.operation,
                &json!({
                    "PredictorName": format!("horizon_{horizon}"),
                    "ForecastHorizon": horizon,
                    "ForecastFrequency": "D",
                    "DataConfig": {"DatasetGroupArn": group_arn},
                }),
            )
            .await;
        assert_eq!(
            status, refusal.status,
            "case {:?}: expected {}, got {status} with {body}",
            refusal.label, refusal.status
        );
        assert_eq!(
            error_code(&body),
            refusal.code,
            "case {:?}: expected {}, got {body}",
            refusal.label,
            refusal.code
        );
    }

    // The documented horizon (2 of the 8 time-steps) is accepted, and the SECOND
    // forecast of the same name is not.
    let predictor = emulator
        .control_ok(
            "CreateAutoPredictor",
            &json!({
                "PredictorName": "refusals_predictor",
                "ForecastHorizon": 2,
                "ForecastFrequency": "D",
                "DataConfig": {"DatasetGroupArn": group_arn},
            }),
        )
        .await;
    let predictor_arn = predictor["PredictorArn"].as_str().unwrap_or_default().to_string();
    settle().await;
    emulator
        .control_ok(
            "CreateForecast",
            &json!({"ForecastName": "refusals_forecast", "PredictorArn": predictor_arn}),
        )
        .await;
    let (status, body) = emulator
        .control(
            "CreateForecast",
            &json!({"ForecastName": "refusals_forecast", "PredictorArn": predictor_arn}),
        )
        .await;
    assert_eq!(
        status, 403,
        "expected a repeated forecast name to be refused 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceAlreadyExistsException",
        "expected ResourceAlreadyExistsException for a repeated forecast name, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The publication's own bound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_forecast_past_the_point_bound_is_refused_rather_than_published() {
    let emulator = Emulator::new();
    let dataset = emulator
        .control_ok(
            "CreateDataset",
            &json!({
                "DatasetName": "bounded_data",
                "Domain": "RETAIL",
                "DatasetType": "TARGET_TIME_SERIES",
                "DataFrequency": "D",
                "Schema": schema_json(RETAIL_TARGET),
            }),
        )
        .await;
    let dataset_arn = dataset["DatasetArn"].as_str().unwrap_or_default().to_string();
    let group = emulator
        .control_ok(
            "CreateDatasetGroup",
            &json!({
                "DatasetGroupName": "bounded_group",
                "Domain": "RETAIL",
                "DatasetArns": [dataset_arn],
            }),
        )
        .await;
    let group_arn = group["DatasetGroupArn"].as_str().unwrap_or_default().to_string();

    // A history that would make the PRODUCT of the per-factor bounds explode: one long
    // series, so a 400-step horizon is within 1/4 of the training length, plus 150
    // single-observation series that would each carry the whole horizon. 151 series x 1
    // statistic x 400 steps = 60,400 points, past the 50,000 one forecast may hold.
    let mut history = String::new();
    for day in 0..1600u32 {
        history.push_str(&format!(
            "anchor,{},{}\n",
            daily_timestamp(day),
            10 + day % 7
        ));
    }
    for item in 0..150u32 {
        history.push_str(&format!("item_{item},{},{}\n", daily_timestamp(0), 5 + item));
    }
    emulator.upload("bounded/history.csv", &history).await;
    emulator
        .control_ok(
            "CreateDatasetImportJob",
            &json!({
                "DatasetImportJobName": "bounded_import",
                "DatasetArn": dataset_arn,
                "TimestampFormat": "yyyy-MM-dd",
                "DataSource": {"S3Config": {
                    "Path": format!("s3://{BUCKET}/bounded/history.csv"),
                    "RoleArn": arn("role/forecast_access"),
                }},
            }),
        )
        .await;
    settle().await;

    let predictor = emulator
        .control_ok(
            "CreateAutoPredictor",
            &json!({
                "PredictorName": "bounded_predictor",
                "ForecastHorizon": 400,
                "ForecastFrequency": "D",
                "ForecastTypes": ["0.5"],
                "DataConfig": {"DatasetGroupArn": group_arn},
            }),
        )
        .await;
    let predictor_arn = predictor["PredictorArn"].as_str().unwrap_or_default().to_string();
    settle().await;

    let (status, body) = emulator
        .control(
            "CreateForecast",
            &json!({"ForecastName": "bounded_forecast", "PredictorArn": predictor_arn}),
        )
        .await;
    assert_eq!(
        status, 409,
        "expected a forecast past the point bound to be refused 409, got {status} with \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "expected LimitExceededException for a forecast past the point bound, got {body}"
    );
    assert!(
        message(&body).contains("prediction points"),
        "expected the refusal to name the prediction-point bound, got {body}"
    );
    // Nothing was published: the bound is checked before a single point is built.
    let (status, body) = emulator
        .runtime(
            "QueryForecast",
            &json!({
                "ForecastArn": arn("forecast/bounded_forecast"),
                "Filters": {"item_id": "anchor"},
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected the refused forecast to be absent from the runtime store, got {status} \
         with {body}"
    );

    // The same predictor over a selector naming ONE series is inside the bound (400
    // points), so the bound refuses what is too large rather than the operation.
    emulator
        .upload("bounded/identifiers.csv", "anchor\n")
        .await;
    let answer = emulator
        .control_ok(
            "CreateForecast",
            &json!({
                "ForecastName": "bounded_one_series",
                "PredictorArn": predictor_arn,
                "TimeSeriesSelector": {"TimeSeriesIdentifiers": {
                    "DataSource": {"S3Config": {
                        "Path": format!("s3://{BUCKET}/bounded/identifiers.csv"),
                        "RoleArn": arn("role/forecast_access"),
                    }},
                    "Schema": {"Attributes": [{"AttributeName": "item_id", "AttributeType": "string"}]},
                    "Format": "CSV",
                }},
            }),
        )
        .await;
    let forecast_arn = answer["ForecastArn"].as_str().unwrap_or_default().to_string();
    settle().await;
    let (status, body) = emulator
        .runtime(
            "QueryForecast",
            &json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "anchor"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the selected single-series forecast to be queryable, got {status} with \
         {body}"
    );
    let answered = predictions(&body);
    let points = answered
        .get("p50")
        .unwrap_or_else(|| panic!("no p50 in {answered:?}"));
    assert_eq!(
        points.len(),
        400,
        "expected one point per horizon step, got {}",
        points.len()
    );
}

/// The `yyyy-MM-dd` date `day` days after 2015-01-01, for the long history above.
///
/// `chrono` is this crate's own calendar dependency (`src/state/series.rs` reads one
/// through it), so the test steps dates with it rather than with a second hand-written
/// leap-year rule.
fn daily_timestamp(day: u32) -> String {
    let start = ::chrono::NaiveDate::from_ymd_opt(2015, 1, 1).expect("2015-01-01 is a date");
    (start + ::chrono::Days::new(u64::from(day)))
        .format("%Y-%m-%d")
        .to_string()
}
