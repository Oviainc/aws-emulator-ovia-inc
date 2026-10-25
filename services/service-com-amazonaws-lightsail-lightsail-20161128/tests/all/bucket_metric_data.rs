// SPDX-License-Identifier: CC0-1.0

//! `GetBucketMetricData` driven over the wire, through the same front door a
//! client uses: the Lightsail calls as `POST /` with `X-Amz-Target:
//! Lightsail_20161128.<Operation>` and an `awsJson1_1` body, the object calls as
//! the path-style S3 requests a client signed with a bucket access key sends, both
//! dispatched by the multi-service router — so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than bypassed.
//!
//! The rule under test: a bucket's two metrics are a COUNT and a SIZE of what the
//! bucket itself holds — "calculated by counting all objects in the bucket
//! (including object versions)" and "by summing the size of all objects in the
//! bucket (including object versions)" — so each is MEASURED in the object store
//! that serves the bucket's objects, reported as one datapoint placed in the
//! period window the measurement falls in, filled only with the statistics the
//! request named and carrying the metric's published unit. A window the
//! measurement does not fall in holds no datapoint; the call writes nothing a
//! later caller can see; and a request that describes no series — an inverted
//! window, no statistic, a unit the metric is not published in — or a name no
//! bucket of the request's region answers to is refused.
//!
//! Both services are mounted on ONE shared engine, exactly as the router mounts
//! them, so what is exercised is the hand-off between the control plane and the
//! object store rather than either store directly.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix the Lightsail service routes on (`OPERATIONS.md`,
/// "Wire protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint every request here reaches the emulator on (its `Host`).
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The bundle every bucket here is created on — the cheapest published plan.
const BUNDLE_ID: &str = "small_1_0";

/// Every metric a bucket publishes, with the unit it is published in.
///
/// Written out here independently of the service's own code, so these rows pin the
/// published list rather than restating what the handler holds.
///
/// SOURCE: the modelled `BucketMetricName` enum
/// (`smithy_query(query="shape",shape="BucketMetricName")`), whose values are
/// exactly the entries the modelled `GetBucketMetricDataRequest$metricName`
/// documentation lists, each with the "published unit" that entry states —
/// `BucketSizeBytes` in `Bytes`, `NumberOfObjects` in `Count`.
const PUBLISHED_METRICS: &[(&str, &str)] = &[("BucketSizeBytes", "Bytes"), ("NumberOfObjects", "Count")];

/// The bounds of the modelled `MetricPeriod` range (`@range(min: 60, max: 86400)`
/// on `MetricPeriod`).
const MIN_PERIOD: i64 = 60;
/// See [`MIN_PERIOD`].
const MAX_PERIOD: i64 = 86_400;

/// The granularities a request may ask for: both published bounds of the modelled
/// `MetricPeriod` range, the 86400 seconds the modelled `period` member calls the
/// granularity bucket storage metrics are reported at, and two more between the
/// bounds.
const PERIODS: &[i64] = &[MIN_PERIOD, 300, 3_600, MAX_PERIOD];

/// Every member of the modelled `MetricStatistic` enum
/// (`smithy_query(query="shape",shape="MetricStatistic")`), which are the
/// statistics a request may name.
const EVERY_STATISTIC: &[&str] = &["Minimum", "Maximum", "Sum", "Average", "SampleCount"];

/// Statistic sets a request may ask for, differing along the count, the members
/// and the order: one statistic, several, every member of the enum, and the same
/// set reversed.
const STATISTIC_SETS: &[&[&str]] = &[
    &["Maximum"],
    &["Average", "SampleCount"],
    &["Minimum", "Maximum", "Sum", "Average", "SampleCount"],
    &["SampleCount", "Average", "Sum", "Maximum", "Minimum"],
];

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// Lightsail and Amazon S3 mounted on ONE shared engine, the way the router mounts
/// every service: the object store the Lightsail control plane provisions a bucket
/// in is the store the S3 data plane serves, and the store these metrics are
/// measured in.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target` (and by the
        // credential scope's service field for the object calls), and the
        // `Authorization` header below carries only that scope, which is what
        // names the region a Lightsail call is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Call a Lightsail `operation` in `region` with `body`, answering its HTTP
    /// status and parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization(region, "lightsail"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call a Lightsail `operation` in `region` and require `want_status`,
    /// answering the body.
    async fn ok(&self, region: &str, operation: &str, body: Value, want_status: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want_status,
            "case {operation} in {region}: expected HTTP {want_status}, got {status} {parsed:?}"
        );
        parsed
    }

    /// One object-data-plane request — the path-style S3 call a client sends to
    /// the bucket's announced address. Answers its status.
    async fn object(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", authorization("us-east-1", "s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request is well formed");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// `PUT /<bucket>/<key>` — S3's `PutObject`, which answers 200.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        let status = self.object("PUT", &format!("/{bucket}/{key}"), body).await;
        assert_eq!(
            status, 200,
            "case PutObject {bucket}/{key}: expected HTTP 200, got {status}"
        );
    }

    /// `DELETE /<bucket>/<key>` — S3's `DeleteObject`, which answers 204.
    async fn delete_object(&self, bucket: &str, key: &str) {
        let status = self
            .object("DELETE", &format!("/{bucket}/{key}"), b"")
            .await;
        assert_eq!(
            status, 204,
            "case DeleteObject {bucket}/{key}: expected HTTP 204, got {status}"
        );
    }

    /// Create the Lightsail bucket `name` in `region`, with object versioning
    /// enabled when `versioned`.
    async fn create_bucket(&self, region: &str, name: &str, versioned: bool) {
        self.ok(
            region,
            "CreateBucket",
            json!({
                "bucketName": name,
                "bundleId": BUNDLE_ID,
                "enableObjectVersioning": versioned,
            }),
            200,
        )
        .await;
    }

    /// The stored bucket `name` in `region`, as `GetBuckets` reports it — the
    /// reader this suite round-trips the record through.
    async fn read_bucket(&self, region: &str, name: &str) -> Value {
        let body = self
            .ok(region, "GetBuckets", json!({"bucketName": name}), 200)
            .await;
        let buckets = body["buckets"]
            .as_array()
            .unwrap_or_else(|| panic!("case reading {name} in {region}: expected an array, got {body:?}"))
            .clone();
        assert_eq!(
            buckets.len(),
            1,
            "case reading {name} in {region}: expected exactly the one named bucket, got {buckets:?}"
        );
        buckets[0].clone()
    }

    /// `GetBucketMetricData` with the [`request`] body `body`.
    async fn metric_data(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "GetBucketMetricData", body).await
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the
/// routing key the router claims a request by — and `region`, which is the Region
/// a Lightsail call is made in. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off, so nothing here is
/// verified.
fn authorization(region: &str, service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One `GetBucketMetricData` request body.
fn request(
    bucket_name: &str,
    metric_name: &str,
    unit: &str,
    period: i64,
    start_time: i64,
    end_time: i64,
    statistics: &[&str],
) -> Value {
    json!({
        "bucketName": bucket_name,
        "metricName": metric_name,
        "unit": unit,
        "period": period,
        "startTime": start_time,
        "endTime": end_time,
        "statistics": statistics,
    })
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The datapoints an answer carries, or a panic naming `case`.
fn datapoints(case: &str, body: &Value) -> Vec<Value> {
    body["metricData"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a metricData array, got {body:?}"))
        .clone()
}

/// Now, in whole epoch seconds — the clock the handler measures against.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch second fits an i64")
}

/// Assert that `datapoint` carries `value` under exactly the `statistics` the
/// request named, the metric's published `unit`, and a `timestamp` that is the
/// start of the `period` window the measuring instant fell in.
fn assert_datapoint(
    case: &str,
    datapoint: &Value,
    value: f64,
    statistics: &[&str],
    unit: &str,
    period: i64,
    taken_from: i64,
) {
    for statistic in EVERY_STATISTIC {
        // `SampleCount` is the number of observations in the window — one, the
        // measurement this call took; every other statistic of a single
        // observation is that observation.
        let expected = if *statistic == "SampleCount" {
            1.0
        } else {
            value
        };
        let member = match *statistic {
            "Minimum" => "minimum",
            "Maximum" => "maximum",
            "Sum" => "sum",
            "Average" => "average",
            _ => "sampleCount",
        };
        if statistics.contains(statistic) {
            assert_eq!(
                datapoint[member].as_f64(),
                Some(expected),
                "case {case}: the requested statistic {statistic} must report {expected}, \
                 got {datapoint:?}"
            );
        } else {
            assert!(
                datapoint.get(member).is_none_or(Value::is_null),
                "case {case}: the statistic {statistic} was not asked for, so it must be \
                 left out, got {datapoint:?}"
            );
        }
    }
    assert_eq!(
        datapoint["unit"].as_str(),
        Some(unit),
        "case {case}: the datapoint must carry the metric's published unit, got {datapoint:?}"
    );
    let timestamp = datapoint["timestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a timestamp, got {datapoint:?}"));
    let window_start = timestamp as i64;
    assert_eq!(
        window_start % period,
        0,
        "case {case}: the timestamp must be the start of a {period}-second window, \
         got {timestamp}"
    );
    assert!(
        window_start <= now_secs() && window_start + period > taken_from,
        "case {case}: the window starting at {window_start} must be the one the measurement \
         (taken at or after {taken_from}) fell in, at period {period}"
    );
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One bucket to fill and then measure. The rows differ along everything the rule
/// generalises over: the region the calls are made in, the form and length of the
/// bucket name, whether the bucket keeps object versions, how many objects it
/// holds, the form of their keys, the size of their payloads, and whether a key
/// was overwritten or deleted.
///
/// `objects` and `bytes` are stated here rather than recomputed from the rows, so
/// they pin the measurement independently: an object version is an object the
/// bucket still holds, a write that keeps no version replaces what it displaces,
/// and a delete marker is not an object (it holds no bytes).
struct BucketRow {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    versioned: bool,
    /// `(key, payload)` pairs written in order.
    puts: &'static [(&'static str, &'static str)],
    /// Keys deleted, in order, after every write.
    deletes: &'static [&'static str],
    objects: f64,
    bytes: f64,
}

const BUCKET_ROWS: &[BucketRow] = &[
    BucketRow {
        case: "a bucket holding nothing at all",
        region: "us-east-1",
        name: "metrics-empty-store",
        versioned: false,
        puts: &[],
        deletes: &[],
        objects: 0.0,
        bytes: 0.0,
    },
    BucketRow {
        case: "one short object under a flat key",
        region: "us-west-2",
        name: "metrics-one-object",
        versioned: false,
        puts: &[("readme.txt", "lightsail")],
        deletes: &[],
        objects: 1.0,
        bytes: 9.0,
    },
    BucketRow {
        case: "three objects of different key forms and payload sizes, one empty",
        region: "eu-west-1",
        name: "metrics-several-objects",
        versioned: false,
        puts: &[
            ("a", "x"),
            ("logs/2026/10/24/app.log", "GET / 200\n"),
            ("empty-object", ""),
        ],
        deletes: &[],
        objects: 3.0,
        bytes: 11.0,
    },
    BucketRow {
        case: "a key overwritten in a bucket that keeps no versions",
        region: "ap-northeast-1",
        name: "metrics-overwritten-unversioned",
        versioned: false,
        puts: &[("report", "first"), ("report", "second-write")],
        deletes: &[],
        objects: 1.0,
        bytes: 12.0,
    },
    BucketRow {
        case: "a key overwritten in a versioning-enabled bucket, which keeps both versions",
        region: "us-east-1",
        name: "metrics-overwritten-versioned",
        versioned: true,
        puts: &[("report", "first"), ("report", "second-write")],
        deletes: &[],
        objects: 2.0,
        bytes: 17.0,
    },
    BucketRow {
        case: "a delete marker laid over an object in a versioning-enabled bucket",
        region: "us-west-2",
        name: "metrics-delete-marker-over-an-object",
        versioned: true,
        puts: &[("kept", "payload")],
        deletes: &["kept"],
        objects: 1.0,
        bytes: 7.0,
    },
    BucketRow {
        case: "a kibibyte of payload spread over two keys",
        region: "eu-west-1",
        name: "metrics-kibibyte-payloads",
        versioned: false,
        puts: &[
            ("archive.tar.gz-001", LONG_PAYLOAD),
            ("archive.tar.gz-002", LONG_PAYLOAD),
        ],
        deletes: &[],
        objects: 2.0,
        bytes: 2_048.0,
    },
];

/// A 1024-byte payload, so a row spans a size no short string reaches: 64 runs of
/// 16 characters, checked below to be exactly the length the rows state.
const LONG_PAYLOAD: &str = concat!(
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
    "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp", "pppppppppppppppp",
);

const _: () = assert!(LONG_PAYLOAD.len() == 1_024);

/// Fill the bucket `row` describes, creating it first.
async fn fill(emulator: &Emulator, row: &BucketRow) {
    emulator
        .create_bucket(row.region, row.name, row.versioned)
        .await;
    for (key, payload) in row.puts {
        emulator
            .put_object(row.name, key, payload.as_bytes())
            .await;
    }
    for key in row.deletes {
        emulator.delete_object(row.name, key).await;
    }
}

/// RULE: each published metric is MEASURED in the object store that serves the
/// bucket's objects — `NumberOfObjects` counting every object version the bucket
/// holds and `BucketSizeBytes` summing their sizes — and reported as one
/// datapoint, carrying the metric's published unit, timestamped at the start of
/// the period window the measurement fell in, and filled with exactly the
/// statistics the request named.
#[tokio::test]
async fn every_published_metric_reports_what_the_bucket_actually_holds() {
    let emulator = Emulator::new();
    let taken_from = now_secs();

    for row in BUCKET_ROWS {
        fill(&emulator, row).await;
    }

    for (index, row) in BUCKET_ROWS.iter().enumerate() {
        for (offset, (metric_name, unit)) in PUBLISHED_METRICS.iter().enumerate() {
            // The cases differ along the granularity and the statistic set as
            // well as the metric and the bucket: the measurement is the same
            // whichever of them the request carries, and only its placement and
            // the members filled change.
            let step = index + offset;
            let period = PERIODS[step % PERIODS.len()];
            let statistics = STATISTIC_SETS[step % STATISTIC_SETS.len()];
            let expected = if *metric_name == "BucketSizeBytes" {
                row.bytes
            } else {
                row.objects
            };
            let case = format!(
                "{} — {metric_name} at period {period} for {statistics:?}",
                row.case
            );

            // A window spanning two days either side of now, so the window the
            // measurement falls in lies inside it at every granularity.
            let (status, body) = emulator
                .metric_data(
                    row.region,
                    request(
                        row.name,
                        metric_name,
                        unit,
                        period,
                        taken_from - 2 * MAX_PERIOD,
                        now_secs() + 2 * MAX_PERIOD,
                        statistics,
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            assert_eq!(
                body["metricName"].as_str(),
                Some(*metric_name),
                "case {case}: expected the requested metric name back, got {body:?}"
            );
            let data = datapoints(&case, &body);
            assert_eq!(
                data.len(),
                1,
                "case {case}: expected the one measurement this store can take, got {data:?}"
            );
            assert_datapoint(
                &case,
                &data[0],
                expected,
                statistics,
                unit,
                period,
                taken_from,
            );
        }
    }
}

/// RULE: the series answers what the bucket holds NOW — a later write, overwrite
/// or delete moves both metrics, each by what that object was.
#[tokio::test]
async fn the_series_follows_what_the_bucket_holds() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let bucket = "metrics-following-store";
    emulator.create_bucket(region, bucket, false).await;

    // What to do before each measurement, and the objects and bytes the bucket
    // then holds.
    struct Step {
        case: &'static str,
        put: Option<(&'static str, &'static str)>,
        delete: Option<&'static str>,
        objects: f64,
        bytes: f64,
    }
    let steps: &[Step] = &[
        Step {
            case: "a bucket nothing was written to",
            put: None,
            delete: None,
            objects: 0.0,
            bytes: 0.0,
        },
        Step {
            case: "one object written",
            put: Some(("one.txt", "0123456789")),
            delete: None,
            objects: 1.0,
            bytes: 10.0,
        },
        Step {
            case: "a second object written",
            put: Some(("two.txt", "abc")),
            delete: None,
            objects: 2.0,
            bytes: 13.0,
        },
        Step {
            case: "the first object overwritten with a shorter payload",
            put: Some(("one.txt", "0")),
            delete: None,
            objects: 2.0,
            bytes: 4.0,
        },
        Step {
            case: "the second object deleted",
            put: None,
            delete: Some("two.txt"),
            objects: 1.0,
            bytes: 1.0,
        },
        Step {
            case: "the last object deleted",
            put: None,
            delete: Some("one.txt"),
            objects: 0.0,
            bytes: 0.0,
        },
    ];

    for step in steps {
        if let Some((key, payload)) = step.put {
            emulator.put_object(bucket, key, payload.as_bytes()).await;
        }
        if let Some(key) = step.delete {
            emulator.delete_object(bucket, key).await;
        }
        for (metric_name, unit) in PUBLISHED_METRICS {
            let expected = if *metric_name == "BucketSizeBytes" {
                step.bytes
            } else {
                step.objects
            };
            let taken_from = now_secs();
            let (status, body) = emulator
                .metric_data(
                    region,
                    request(
                        bucket,
                        metric_name,
                        unit,
                        MAX_PERIOD,
                        taken_from - MAX_PERIOD,
                        taken_from + MAX_PERIOD,
                        &["Maximum", "Average", "SampleCount"],
                    ),
                )
                .await;
            let case = format!("{} — {metric_name}", step.case);
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            let data = datapoints(&case, &body);
            assert_eq!(
                data.len(),
                1,
                "case {case}: expected one measurement, got {data:?}"
            );
            assert_eq!(
                data[0]["maximum"].as_f64(),
                Some(expected),
                "case {case}: expected {expected}, got {:?}",
                data[0]
            );
        }
    }
}

/// RULE: a datapoint belongs to the period window the measurement fell in, so a
/// requested window that shares no instant with that window carries none — one
/// that ended before this emulator's clock reached it, or one that has not begun.
#[tokio::test]
async fn a_window_the_measurement_does_not_fall_in_carries_no_datapoint() {
    let emulator = Emulator::new();
    let region = "eu-west-1";
    let bucket = "metrics-window-placement";
    emulator.create_bucket(region, bucket, false).await;
    emulator.put_object(bucket, "probe", b"probe").await;

    let now = now_secs();
    // (case, startTime, endTime, whether the window holds the measurement)
    let windows: &[(&str, i64, i64, bool)] = &[
        ("a window years before this emulator ran", 1_000_000_000, 1_000_086_400, false),
        ("a window ending a day before now", now - 2 * MAX_PERIOD, now - MAX_PERIOD, false),
        ("a window entirely in the future", now + 10 * MAX_PERIOD, now + 11 * MAX_PERIOD, false),
        ("a window around now", now - MAX_PERIOD, now + MAX_PERIOD, true),
        ("a window from now into the future", now, now + MAX_PERIOD, true),
    ];

    for (case, start_time, end_time, holds) in windows {
        for (metric_name, unit) in PUBLISHED_METRICS {
            let case = format!("{case} — {metric_name}");
            let (status, body) = emulator
                .metric_data(
                    region,
                    request(
                        bucket,
                        metric_name,
                        unit,
                        MIN_PERIOD,
                        *start_time,
                        *end_time,
                        &["Maximum"],
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            let data = datapoints(&case, &body);
            assert_eq!(
                data.len(),
                usize::from(*holds),
                "case {case}: expected {} datapoint(s), got {data:?}",
                usize::from(*holds)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reading changes nothing
// ---------------------------------------------------------------------------

/// RULE: reading metrics reports; it does not change what a later caller sees.
/// Repeated reads answer the same measurement, and both the bucket record
/// round-tripped through `GetBuckets` and the bucket's contents are exactly what
/// they were before the reads.
#[tokio::test]
async fn reading_metrics_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let bucket = "metrics-quiet-store";
    emulator.create_bucket(region, bucket, false).await;
    emulator.put_object(bucket, "kept.txt", b"kept bytes").await;
    let before = emulator.read_bucket(region, bucket).await;

    let mut answers = Vec::new();
    for round in 0..2 {
        for (metric_name, unit) in PUBLISHED_METRICS {
            let case = format!("round {round} of {metric_name}");
            let taken_from = now_secs();
            let (status, body) = emulator
                .metric_data(
                    region,
                    request(
                        bucket,
                        metric_name,
                        unit,
                        MAX_PERIOD,
                        taken_from - MAX_PERIOD,
                        taken_from + MAX_PERIOD,
                        &["Maximum", "Sum"],
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            answers.push((case, body));
        }
    }
    let repeated = answers.len() / 2;
    for index in 0..repeated {
        let (case, first) = &answers[index];
        let (_, second) = &answers[index + repeated];
        assert_eq!(
            first, second,
            "case {case}: a repeated read must answer the same thing, got {second:?}"
        );
    }

    let after = emulator.read_bucket(region, bucket).await;
    assert_eq!(
        before, after,
        "case the bucket record after the reads: expected {before:?}, got {after:?}"
    );
    // The contents are untouched too: the measurement is still the one object of
    // ten bytes that was written.
    let taken_from = now_secs();
    let (_, body) = emulator
        .metric_data(
            region,
            request(
                bucket,
                "NumberOfObjects",
                "Count",
                MAX_PERIOD,
                taken_from - MAX_PERIOD,
                taken_from + MAX_PERIOD,
                &["Maximum"],
            ),
        )
        .await;
    let data = datapoints("the contents after the reads", &body);
    assert_eq!(
        data.first().and_then(|point| point["maximum"].as_f64()),
        Some(1.0),
        "case the contents after the reads: expected the one object still there, got {data:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals the handler owns
// ---------------------------------------------------------------------------

/// RULE: a request the model admits but that describes no series — a window whose
/// end does not follow its start, a request naming no statistic, or a unit the
/// metric is not published in — is refused with the modelled 400
/// `InvalidInputException`, and refusing it changes nothing.
#[tokio::test]
async fn a_request_that_describes_no_series_is_refused_as_invalid_input() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let bucket = "metrics-refusal-store";
    emulator.create_bucket(region, bucket, false).await;
    emulator.put_object(bucket, "probe", b"probe").await;
    let before = emulator.read_bucket(region, bucket).await;

    let now = now_secs();
    let rows: &[(&str, Value)] = &[
        (
            "a window that ends where it starts",
            request(bucket, "BucketSizeBytes", "Bytes", 300, now, now, &["Maximum"]),
        ),
        (
            "a window that ends one second before it starts",
            request(
                bucket,
                "NumberOfObjects",
                "Count",
                MIN_PERIOD,
                now + 1,
                now,
                &["Average"],
            ),
        ),
        (
            "a window that ends a day before it starts",
            request(
                bucket,
                "BucketSizeBytes",
                "Bytes",
                MAX_PERIOD,
                now + MAX_PERIOD,
                now,
                &["Sum"],
            ),
        ),
        (
            "a BucketSizeBytes request naming no statistic",
            request(
                bucket,
                "BucketSizeBytes",
                "Bytes",
                300,
                now - 3_600,
                now,
                &[],
            ),
        ),
        (
            "a NumberOfObjects request naming no statistic",
            request(
                bucket,
                "NumberOfObjects",
                "Count",
                MAX_PERIOD,
                now - MAX_PERIOD,
                now,
                &[],
            ),
        ),
        (
            "BucketSizeBytes asked for in the unit the other metric is published in",
            request(
                bucket,
                "BucketSizeBytes",
                "Count",
                MAX_PERIOD,
                now - MAX_PERIOD,
                now,
                &["Maximum"],
            ),
        ),
        (
            "NumberOfObjects asked for in the unit the other metric is published in",
            request(
                bucket,
                "NumberOfObjects",
                "Bytes",
                MAX_PERIOD,
                now - MAX_PERIOD,
                now,
                &["Average"],
            ),
        ),
        (
            "a metric asked for in a unit no bucket metric is published in",
            request(
                bucket,
                "NumberOfObjects",
                "Percent",
                300,
                now - 3_600,
                now,
                &["Average"],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = emulator
            .call(region, "GetBucketMetricData", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "case {case}: expected the wire error InvalidInputException, got {answered:?}"
        );
        assert!(
            answered["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message saying what was wrong, got {answered:?}"
        );
    }

    let after = emulator.read_bucket(region, bucket).await;
    assert_eq!(
        before, after,
        "case the bucket record after every refusal: expected {before:?}, got {after:?}"
    );
}

/// RULE: a name no bucket of the request's region answers to — never created, or
/// created in a different region — earns the service's `DoesNotExist` envelope:
/// HTTP 400, the wire error `NotFoundException`, the body `code` `DoesNotExist`,
/// and a message naming the resource type and the requested name.
#[tokio::test]
async fn a_name_no_bucket_answers_to_earns_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    // A bucket that exists, but in ANOTHER region: the lookup is scoped to the
    // region the request is made in.
    let elsewhere = "metrics-other-region-store";
    emulator.create_bucket("eu-west-1", elsewhere, false).await;

    // Names differing along what the rule generalises over — length, digits and
    // separators — plus the one that exists only elsewhere.
    let names = [
        "abc",
        "never-created-store",
        "store-0123456789",
        "a-bucket-for-a-tier-that-was-never-built",
        elsewhere,
    ];
    let now = now_secs();
    for name in names {
        let case = format!("GetBucketMetricData on {name:?} in {region}");
        let (status, body) = emulator
            .metric_data(
                region,
                request(
                    name,
                    "NumberOfObjects",
                    "Count",
                    MAX_PERIOD,
                    now - MAX_PERIOD,
                    now + MAX_PERIOD,
                    &["Average"],
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the wire error NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Bucket does not exist: {name}").as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals the modelled constraints own
// ---------------------------------------------------------------------------

/// RULE: a value outside a modelled constraint is refused by the boundary walk
/// with its own 400 `ValidationException`, before the handler runs — the
/// `MetricPeriod` range one past each published bound, and a name outside each
/// enum this input reaches — while each published bound of the range is itself
/// accepted.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_by_the_boundary_walk() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let bucket = "metrics-boundary-store";
    emulator.create_bucket(region, bucket, false).await;
    emulator.put_object(bucket, "probe", b"probe!").await;

    let now = now_secs();
    let rows: &[(&str, Value)] = &[
        (
            "a period one second below the range",
            request(
                bucket,
                "BucketSizeBytes",
                "Bytes",
                MIN_PERIOD - 1,
                now - 3_600,
                now,
                &["Maximum"],
            ),
        ),
        (
            "a period one second above the range",
            request(
                bucket,
                "BucketSizeBytes",
                "Bytes",
                MAX_PERIOD + 1,
                now - 90_000,
                now,
                &["Maximum"],
            ),
        ),
        (
            "a metric name outside the modelled enum",
            request(
                bucket,
                "CPUUtilization",
                "Percent",
                300,
                now - 3_600,
                now,
                &["Average"],
            ),
        ),
        (
            "a unit outside the modelled enum",
            request(
                bucket,
                "BucketSizeBytes",
                "Petabytes",
                300,
                now - 3_600,
                now,
                &["Maximum"],
            ),
        ),
        (
            "a statistic outside the modelled enum",
            request(
                bucket,
                "NumberOfObjects",
                "Count",
                300,
                now - 3_600,
                now,
                &["Median"],
            ),
        ),
        (
            "a bucket name one character shorter than the pattern admits",
            request(
                "ab",
                "NumberOfObjects",
                "Count",
                300,
                now - 3_600,
                now,
                &["Average"],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = emulator
            .call(region, "GetBucketMetricData", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: expected the boundary walk's ValidationException, got {answered:?}"
        );
    }

    // Both published bounds of the range are themselves accepted, and the
    // measurement is reported at each granularity.
    for period in [MIN_PERIOD, MAX_PERIOD] {
        let case = format!("the period bound {period}");
        let taken_from = now_secs();
        let body = emulator
            .ok(
                region,
                "GetBucketMetricData",
                request(
                    bucket,
                    "BucketSizeBytes",
                    "Bytes",
                    period,
                    taken_from - MAX_PERIOD,
                    taken_from + MAX_PERIOD,
                    &["Maximum"],
                ),
                200,
            )
            .await;
        let data = datapoints(&case, &body);
        assert_eq!(
            data.len(),
            1,
            "case {case}: expected the one measurement, got {data:?}"
        );
        assert_datapoint(
            &case,
            &data[0],
            6.0,
            &["Maximum"],
            "Bytes",
            period,
            taken_from,
        );
    }
}
