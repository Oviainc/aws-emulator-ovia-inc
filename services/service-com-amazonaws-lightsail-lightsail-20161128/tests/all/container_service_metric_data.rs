// SPDX-License-Identifier: CC0-1.0

//! `GetContainerServiceMetricData` driven over the wire, through the same front
//! door a client uses: `POST /` with `X-Amz-Target:
//! Lightsail_20161128.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router, so the modelled-constraint walk, the handler and the
//! awsJson error envelope are all exercised rather than bypassed.
//!
//! The rule under test: reading a container service metric is a REPORT on the
//! container service record the store holds in the region the request is made in,
//! and on the observations it holds for that metric. So for every metric the model
//! publishes, over any window at any granularity the model admits and for any
//! statistic set, the answer is HTTP 200 carrying the requested `metricName` and
//! the datapoints the store has — none, because this emulator runs no container
//! and measures nothing on one — the call writes nothing a later caller can see, a
//! window whose end does not follow its start or a request naming no statistic is
//! refused with the modelled 400 `InvalidInputException`, and a name no container
//! service of that region answers to earns the service's `DoesNotExist` envelope.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// Every metric a container service publishes.
///
/// Written out here independently of the service's own code, so these rows pin
/// the published list rather than restating what the handler holds.
///
/// SOURCE: the modelled `ContainerServiceMetricName` enum
/// (`smithy_query(query="shape",shape="ContainerServiceMetricName")`), whose
/// values are exactly the entries the modelled
/// `GetContainerServiceMetricDataRequest$metricName` documentation lists ("Valid
/// container service metric names are listed below…"), each published in
/// `Percent`.
const PUBLISHED_METRICS: &[&str] = &["CPUUtilization", "MemoryUtilization"];

/// The bounds of the modelled `MetricPeriod` range (`@range(min: 60, max:
/// 86400)` on `MetricPeriod`).
const MIN_PERIOD: i64 = 60;
/// See [`MIN_PERIOD`].
const MAX_PERIOD: i64 = 86_400;

/// The granularities a request may ask for: both published bounds of the modelled
/// `MetricPeriod` range, the 300 seconds the modelled `period` member calls the
/// granularity container service metric data is available in, and one more
/// between the bounds.
const PERIODS: &[i64] = &[MIN_PERIOD, 300, 3_600, MAX_PERIOD];

/// Statistic sets a request may ask for, differing along the count, the members
/// and the order: one statistic, several, every member of the modelled
/// `MetricStatistic` enum, and the same set reversed.
const STATISTIC_SETS: &[&[&str]] = &[
    &["Average"],
    &["Maximum", "Minimum"],
    &["Minimum", "Maximum", "Sum", "Average", "SampleCount"],
    &["SampleCount", "Average", "Sum", "Maximum", "Minimum"],
];

/// The instant the windows in this suite start at, in epoch seconds — an
/// arbitrary fixed point, so a case's window is the same on every run.
const WINDOW_START: i64 = 1_760_000_000;

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "127.0.0.1:9297")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
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

    /// Call `operation` in `region` and require `want_status`, answering the body.
    async fn ok(&self, region: &str, operation: &str, body: Value, want_status: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want_status,
            "case {operation} in {region}: expected HTTP {want_status}, got {status} {parsed:?}"
        );
        parsed
    }

    /// Create the container service `name` in `region`. `CreateContainerService`
    /// answers HTTP 201 (its modelled success status).
    async fn create_service(&self, region: &str, name: &str, power: &str, scale: i64) {
        self.ok(
            region,
            "CreateContainerService",
            json!({"serviceName": name, "power": power, "scale": scale}),
            201,
        )
        .await;
    }

    /// The stored container service `name` in `region`, as `GetContainerServices`
    /// reports it — the reader this suite round-trips the record through.
    async fn read_service(&self, region: &str, name: &str) -> Value {
        let body = self
            .ok(
                region,
                "GetContainerServices",
                json!({"serviceName": name}),
                200,
            )
            .await;
        let services = body["containerServices"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case reading {name} in {region}: expected an array, got {body:?}")
            })
            .clone();
        assert_eq!(
            services.len(),
            1,
            "case reading {name} in {region}: expected exactly the one named service, \
             got {services:?}"
        );
        services[0].clone()
    }
}

/// One `GetContainerServiceMetricData` request body.
fn request(
    service_name: &str,
    metric_name: &str,
    period: i64,
    start_time: i64,
    end_time: i64,
    statistics: &[&str],
) -> Value {
    json!({
        "serviceName": service_name,
        "metricName": metric_name,
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
fn metric_data(case: &str, body: &Value) -> Vec<Value> {
    body["metricData"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a metricData array, got {body:?}"))
        .clone()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One container service to create and then read the metrics of. The rows differ
/// along everything the rule generalises over beside the metric: the region the
/// requests are made in, the length and shape of the service name (the shortest
/// the modelled `ContainerServiceName` pattern admits, a hyphenated one, one
/// carrying digits), and the node power and scale the service runs at.
#[derive(Debug, Clone, Copy)]
struct ServiceRow {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    power: &'static str,
    scale: i64,
}

/// The table of container services every metric case below is asked of.
const SERVICE_ROWS: &[ServiceRow] = &[
    ServiceRow {
        case: "a hyphenated name in the default region at nano/1",
        region: "us-east-1",
        name: "metrics-service",
        power: "nano",
        scale: 1,
    },
    ServiceRow {
        case: "the shortest name the pattern admits, in another region",
        region: "us-west-2",
        name: "ab",
        power: "micro",
        scale: 2,
    },
    ServiceRow {
        case: "a name carrying digits, in a European region at a larger power",
        region: "eu-west-1",
        name: "svc-0123456789",
        power: "small",
        scale: 4,
    },
    ServiceRow {
        case: "a long hyphenated name in an Asian region",
        region: "ap-northeast-1",
        name: "containers-for-the-storefront-tier-0",
        power: "medium",
        scale: 3,
    },
];

/// RULE: every metric the model publishes, asked of a stored container service
/// over a window at any granularity the model admits and for any statistic set, is
/// answered with the requested `metricName` and the observations the store holds
/// for it — of which there are none, for every metric, service and window.
#[tokio::test]
async fn every_published_metric_answers_its_own_name_and_the_stored_series() {
    let emulator = Emulator::new();

    for row in SERVICE_ROWS {
        emulator
            .create_service(row.region, row.name, row.power, row.scale)
            .await;
    }

    for (index, row) in SERVICE_ROWS.iter().enumerate() {
        for (offset, metric_name) in PUBLISHED_METRICS.iter().enumerate() {
            // The cases differ along the granularity, the statistic set and the
            // size and placement of the window as well as the metric and the
            // service: the answer is the stored series whichever of them the
            // request carries.
            let step = index + offset;
            let period = PERIODS[step % PERIODS.len()];
            let statistics = STATISTIC_SETS[step % STATISTIC_SETS.len()];
            let start_time = WINDOW_START + (step as i64) * 900;
            let end_time = start_time + period * (step as i64 + 1);
            let case = format!(
                "{} — {metric_name} at period {period} for {:?}",
                row.case, statistics
            );

            let (status, body) = emulator
                .call(
                    row.region,
                    "GetContainerServiceMetricData",
                    request(
                        row.name,
                        metric_name,
                        period,
                        start_time,
                        end_time,
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
            let data = metric_data(&case, &body);
            assert!(
                data.is_empty(),
                "case {case}: this emulator runs no container, so the series must carry no \
                 datapoints, got {data:?}"
            );
        }
    }
}

/// RULE: a window is a window wherever it lies — before the container service was
/// created, around now, or entirely in the future — and the series is still the
/// stored one.
#[tokio::test]
async fn a_window_is_served_wherever_it_lies_relative_to_the_service() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let service = "window-service";
    emulator.create_service(region, service, "nano", 1).await;

    // Epoch seconds: a window long before this service existed, one straddling
    // the recent past, one entirely ahead of any clock this emulator reads, the
    // longest the modelled granularity spans, and the smallest window the
    // ordering rule admits (one second).
    let windows: &[(&str, i64, i64)] = &[
        (
            "a window years before the service existed",
            1_000_000_000,
            1_000_003_600,
        ),
        (
            "a window ending in the recent past",
            1_760_000_000,
            1_760_086_400,
        ),
        (
            "a window entirely in the future",
            4_000_000_000,
            4_000_003_600,
        ),
        (
            "the shortest window the ordering admits",
            1_760_000_000,
            1_760_000_001,
        ),
    ];

    for (case, start_time, end_time) in windows {
        let body = emulator
            .ok(
                region,
                "GetContainerServiceMetricData",
                request(
                    service,
                    "CPUUtilization",
                    MIN_PERIOD,
                    *start_time,
                    *end_time,
                    &["Average", "Maximum"],
                ),
                200,
            )
            .await;
        assert_eq!(
            body["metricName"].as_str(),
            Some("CPUUtilization"),
            "case {case}: expected the requested metric name back, got {body:?}"
        );
        let data = metric_data(case, &body);
        assert!(
            data.is_empty(),
            "case {case}: expected no datapoints, got {data:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Reading changes nothing
// ---------------------------------------------------------------------------

/// RULE: reading metrics reports; it does not change what a later caller sees.
/// The same request answers the same thing twice, and the container service
/// record round-tripped through `GetContainerServices` is exactly what it was
/// before the reads.
#[tokio::test]
async fn reading_metrics_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let service = "quiet-service";
    emulator.create_service(region, service, "micro", 2).await;
    let before = emulator.read_service(region, service).await;

    let mut answers = Vec::new();
    for round in 0..2 {
        for metric_name in PUBLISHED_METRICS {
            let case = format!("round {round} of {metric_name}");
            let answer = emulator
                .ok(
                    region,
                    "GetContainerServiceMetricData",
                    request(
                        service,
                        metric_name,
                        300,
                        WINDOW_START,
                        WINDOW_START + 3_600,
                        &["Average", "Maximum"],
                    ),
                    200,
                )
                .await;
            assert!(
                metric_data(&case, &answer).is_empty(),
                "case {case}: expected no datapoints, got {answer:?}"
            );
            answers.push((case, answer));
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

    let after = emulator.read_service(region, service).await;
    assert_eq!(
        before, after,
        "case the container service record after the reads: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals the handler owns
// ---------------------------------------------------------------------------

/// RULE: a request the model admits but that cannot describe a series — a window
/// whose end does not follow its start, or a request naming no statistic — is
/// refused with the modelled 400 `InvalidInputException`, and refusing it changes
/// nothing.
#[tokio::test]
async fn a_request_that_describes_no_series_is_refused_as_invalid_input() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let service = "refusal-service";
    emulator.create_service(region, service, "nano", 1).await;
    let before = emulator.read_service(region, service).await;

    // One row per way the request fails to describe a series, varied over both
    // published metrics, both granularity bounds and both orderings a non-window
    // can take.
    let rows: &[(&str, Value)] = &[
        (
            "a window that ends where it starts",
            request(
                service,
                "CPUUtilization",
                300,
                WINDOW_START,
                WINDOW_START,
                &["Average"],
            ),
        ),
        (
            "a window that ends one second before it starts",
            request(
                service,
                "MemoryUtilization",
                MIN_PERIOD,
                WINDOW_START + 1,
                WINDOW_START,
                &["Maximum"],
            ),
        ),
        (
            "a window that ends a day before it starts",
            request(
                service,
                "CPUUtilization",
                MAX_PERIOD,
                WINDOW_START + 86_400,
                WINDOW_START,
                &["Sum"],
            ),
        ),
        (
            "a CPUUtilization request naming no statistic",
            request(
                service,
                "CPUUtilization",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &[],
            ),
        ),
        (
            "a MemoryUtilization request naming no statistic",
            request(
                service,
                "MemoryUtilization",
                MAX_PERIOD,
                WINDOW_START,
                WINDOW_START + 86_400,
                &[],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = emulator
            .call(region, "GetContainerServiceMetricData", body.clone())
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

    let after = emulator.read_service(region, service).await;
    assert_eq!(
        before, after,
        "case the container service record after every refusal: expected {before:?}, \
         got {after:?}"
    );
}

/// RULE: a name no container service of the request's region answers to — never
/// created, created in a different region, or created and since deleted — earns
/// the service's `DoesNotExist` envelope: HTTP 400, the wire error
/// `NotFoundException`, the body `code` `DoesNotExist`, and a message naming the
/// resource type and the requested name.
#[tokio::test]
async fn a_name_no_container_service_answers_to_earns_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    // A service that exists, but in ANOTHER region: the lookup is scoped to the
    // region the request is made in.
    let elsewhere = "other-region-service";
    emulator
        .create_service("eu-west-1", elsewhere, "nano", 1)
        .await;

    // Names differing along what the rule generalises over — length, digits and
    // separators — plus the one that exists only elsewhere.
    let names = [
        "ab",
        "never-created",
        "svc-0123456789",
        "containers-for-a-tier-that-was-never-built",
        elsewhere,
    ];
    for name in names {
        let case = format!("GetContainerServiceMetricData on {name:?} in {region}");
        let (status, body) = emulator
            .call(
                region,
                "GetContainerServiceMetricData",
                request(
                    name,
                    "MemoryUtilization",
                    300,
                    WINDOW_START,
                    WINDOW_START + 3_600,
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
            Some(format!("The ContainerService does not exist: {name}").as_str()),
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
    let service = "boundary-service";
    emulator.create_service(region, service, "nano", 1).await;

    let rows: &[(&str, Value)] = &[
        (
            "a period one second below the range",
            request(
                service,
                "CPUUtilization",
                MIN_PERIOD - 1,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Average"],
            ),
        ),
        (
            "a period one second above the range",
            request(
                service,
                "CPUUtilization",
                MAX_PERIOD + 1,
                WINDOW_START,
                WINDOW_START + 90_000,
                &["Average"],
            ),
        ),
        (
            "a metric name outside the modelled enum",
            request(
                service,
                "DiskQueueLength",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Average"],
            ),
        ),
        (
            "a statistic outside the modelled enum",
            request(
                service,
                "CPUUtilization",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Median"],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = emulator
            .call(region, "GetContainerServiceMetricData", body.clone())
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

    // Both published bounds of the range are themselves accepted.
    for period in [MIN_PERIOD, MAX_PERIOD] {
        let case = format!("the period bound {period}");
        let body = emulator
            .ok(
                region,
                "GetContainerServiceMetricData",
                request(
                    service,
                    "CPUUtilization",
                    period,
                    WINDOW_START,
                    WINDOW_START + period,
                    &["Average"],
                ),
                200,
            )
            .await;
        assert!(
            metric_data(&case, &body).is_empty(),
            "case {case}: expected an empty series, got {body:?}"
        );
    }
}
