// SPDX-License-Identifier: CC0-1.0

//! `GetLoadBalancerMetricData` driven over the wire, through the same front door
//! a client uses: `POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>`
//! and an `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rule under test: reading a load balancer metric is a REPORT on the load
//! balancer record the store holds in the Region the request is made in, and on
//! the observations it holds for that metric. So for every metric the service
//! publishes, asked for in the unit that metric is published in, over any window
//! at any granularity the model admits, the answer is HTTP 200 carrying the
//! requested `metricName` and the datapoints the store has — none, because this
//! emulator serves no traffic through a load balancer and runs no health check
//! against a registered instance — the call writes nothing a later caller can
//! see, a metric asked for in a unit it is not published in, a window whose end
//! does not follow its start, or a request naming no statistic is refused with
//! the modelled 400 `InvalidInputException`, and a name no load balancer record
//! of that Region answers to earns the service's `DoesNotExist` envelope.
//!
//! The harness, the instance stand-up and the load balancer stand-up are the ones
//! this test target already exposes.

use serde_json::{json, Value};

use crate::disk_attachment::create_running_instance;
use crate::instance_lifecycle::{error_code, Emulator};
use crate::load_balancer_instance_attachment::{
    create_load_balancer, health_summary, wait_for_load_balancer_state,
};

/// Every metric a load balancer publishes, with the unit its data is published
/// in.
///
/// Written out here independently of the service's own catalog, so these rows
/// pin the published list rather than restating whatever the code holds.
///
/// SOURCE: the modelled `GetLoadBalancerMetricDataRequest$metricName`
/// documentation ("Valid load balancer metric names are listed below, along with
/// the most useful `statistics` to include in your request, and the published
/// `unit` value"), which states one published unit per metric — `Seconds` for the
/// response time, `Count` for every other — and covers every member of the
/// modelled `LoadBalancerMetricName` enum
/// (`smithy_query(query="shape",shape="LoadBalancerMetricName")`).
const PUBLISHED_UNITS: &[(&str, &str)] = &[
    ("ClientTLSNegotiationErrorCount", "Count"),
    ("HealthyHostCount", "Count"),
    ("UnhealthyHostCount", "Count"),
    ("HTTPCode_LB_4XX_Count", "Count"),
    ("HTTPCode_LB_5XX_Count", "Count"),
    ("HTTPCode_Instance_2XX_Count", "Count"),
    ("HTTPCode_Instance_3XX_Count", "Count"),
    ("HTTPCode_Instance_4XX_Count", "Count"),
    ("HTTPCode_Instance_5XX_Count", "Count"),
    ("InstanceResponseTime", "Seconds"),
    ("RejectedConnectionCount", "Count"),
    ("RequestCount", "Count"),
];

/// The bounds of the modelled `MetricPeriod` range (`@range(min: 60, max:
/// 86400)` on `MetricPeriod`).
const MIN_PERIOD: i64 = 60;
/// See [`MIN_PERIOD`].
const MAX_PERIOD: i64 = 86_400;

/// The granularities a request may ask for: both published bounds of the
/// modelled `MetricPeriod` range and two values between them.
const PERIODS: &[i64] = &[MIN_PERIOD, 300, 3_600, MAX_PERIOD];

/// Statistic sets a request may ask for, differing along the count, the members
/// and the order: one statistic, several, every member of the modelled
/// `MetricStatistic` enum (`smithy_query(query="shape",shape="MetricStatistic")`),
/// and the same set reversed.
const STATISTIC_SETS: &[&[&str]] = &[
    &["Average"],
    &["Sum"],
    &["Maximum", "Minimum"],
    &["Minimum", "Maximum", "Sum", "Average", "SampleCount"],
    &["SampleCount", "Average", "Sum", "Maximum", "Minimum"],
];

/// The Regions the cases spread over: a load balancer is a REGIONAL resource, so
/// the Region a request is made in is part of what addresses one.
const REGIONS: &[&str] = &["us-east-1", "us-west-2", "eu-central-1"];

/// The Region a request carrying no credential scope is served in
/// (`crate::state::DEFAULT_REGION`), which is where the harness's unscoped calls
/// — the instance stand-up among them — put their resources.
const DEFAULT_REGION: &str = "us-east-1";

/// The Availability Zone the instances in this suite run in, in
/// [`DEFAULT_REGION`].
const DEFAULT_ZONE: &str = "us-east-1a";

/// The instant the windows in this suite start at, in epoch seconds — an
/// arbitrary fixed point, so a case's window is the same on every run.
const WINDOW_START: i64 = 1_760_000_000;

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The `InstanceHealthState` a registered instance of this store reports — the
/// modelled `initial`, which is neither healthy nor unhealthy, because no health
/// check has run against it.
const INITIAL_HEALTH: &str = "initial";

/// One `GetLoadBalancerMetricData` request body.
fn request(
    load_balancer_name: &str,
    metric_name: &str,
    unit: &str,
    period: i64,
    start_time: i64,
    end_time: i64,
    statistics: &[&str],
) -> Value {
    json!({
        "loadBalancerName": load_balancer_name,
        "metricName": metric_name,
        "unit": unit,
        "period": period,
        "startTime": start_time,
        "endTime": end_time,
        "statistics": statistics,
    })
}

/// Read metric data in `region`, answering the HTTP status and parsed body.
async fn read_metrics(emulator: &Emulator, region: &str, body: Value) -> (u16, Value) {
    emulator
        .call_in(Some(region), "GetLoadBalancerMetricData", body)
        .await
}

/// The datapoints an answer carries, or a panic naming `case`.
fn metric_data(case: &str, body: &Value) -> Vec<Value> {
    body["metricData"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a metricData array, got {body:?}"))
        .clone()
}

/// Stand the load balancer `name` up in `region` and wait until it is `active`,
/// so every case starts from a load balancer the store really holds.
async fn balancer(emulator: &Emulator, region: &str, name: &str) {
    create_load_balancer(emulator, region, name, 80).await;
    wait_for_load_balancer_state(emulator, region, name, "active").await;
}

/// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports it.
async fn record(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
        .clone()
}

// ---------------------------------------------------------------------------
// The published catalog
// ---------------------------------------------------------------------------

/// RULE: every metric a load balancer publishes, asked for in the unit it is
/// published in over a window at any granularity the model admits, is answered
/// with the requested `metricName` and the observations the store holds for it —
/// of which there are none, for every metric, every Region and every window.
#[tokio::test]
async fn every_published_metric_answers_its_own_name_and_the_stored_series() {
    let emulator = Emulator::new();
    for region in REGIONS {
        balancer(&emulator, region, "catalog-lb").await;
    }

    for (index, (metric_name, unit)) in PUBLISHED_UNITS.iter().enumerate() {
        // The rows differ along the Region, the granularity, the statistic set
        // and the size and placement of the window as well as the metric: the
        // answer is the stored series whichever of them the request carries.
        let region = REGIONS[index % REGIONS.len()];
        let period = PERIODS[index % PERIODS.len()];
        let statistics = STATISTIC_SETS[index % STATISTIC_SETS.len()];
        let start_time = WINDOW_START + (index as i64) * 900;
        let end_time = start_time + period * (index as i64 + 1);
        let case = format!("{metric_name} in {unit} at period {period} in {region}");

        let (status, body) = read_metrics(
            &emulator,
            region,
            request(
                "catalog-lb",
                metric_name,
                unit,
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
            "case {case}: this emulator carries no traffic through a load balancer and runs no \
             health check, so the series must carry no datapoints, got {data:?}"
        );
    }
}

/// RULE: a window is a window wherever it lies — before the load balancer was
/// created, around now, or entirely in the future — and the series is still the
/// stored one.
#[tokio::test]
async fn a_window_is_served_wherever_it_lies_relative_to_the_load_balancer() {
    let emulator = Emulator::new();
    let name = "window-lb";
    balancer(&emulator, DEFAULT_REGION, name).await;

    // Epoch seconds: a window long before this load balancer existed, one
    // straddling the recent past, one entirely ahead of any clock this emulator
    // reads, and the smallest window the ordering rule admits (one second).
    let windows: &[(&str, i64, i64)] = &[
        (
            "a window years before the load balancer existed",
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
        let (status, body) = read_metrics(
            &emulator,
            DEFAULT_REGION,
            request(
                name,
                "RequestCount",
                "Count",
                MIN_PERIOD,
                *start_time,
                *end_time,
                &["Sum", "SampleCount"],
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["metricName"].as_str(),
            Some("RequestCount"),
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
// The host-count metrics follow the health checks, which nothing runs
// ---------------------------------------------------------------------------

/// RULE: the healthy and unhealthy host counts are observations of the HEALTH
/// CHECKS a load balancer ran against its targets, and this emulator runs none —
/// which is why every registration stays in the modelled `initial` state,
/// neither healthy nor unhealthy — so registering instances adds no datapoint to
/// either series, however many are registered.
#[tokio::test]
async fn registering_instances_adds_no_host_count_datapoint() {
    let emulator = Emulator::new();
    let name = "hosts-lb";
    balancer(&emulator, DEFAULT_REGION, name).await;

    let host_counts = ["HealthyHostCount", "UnhealthyHostCount"];
    // The registration sets differ along the count: none, one, then two.
    let instances = ["host-one", "host-two"];
    for instance in instances {
        create_running_instance(&emulator, instance, DEFAULT_ZONE).await;
    }

    for registered in 0..=instances.len() {
        if registered > 0 {
            emulator
                .ok_at(
                    None,
                    Some(DEFAULT_REGION),
                    "AttachInstancesToLoadBalancer",
                    json!({
                        "loadBalancerName": name,
                        "instanceNames": [instances[registered - 1]],
                    }),
                )
                .await;
        }
        let summary = health_summary(&emulator, DEFAULT_REGION, name).await;
        assert_eq!(
            summary.len(),
            registered,
            "case {registered} registered instances: expected that many health summary \
             entries, got {summary:?}"
        );
        for entry in &summary {
            assert_eq!(
                entry["instanceHealth"].as_str(),
                Some(INITIAL_HEALTH),
                "case {registered} registered instances: a registration no health check has \
                 run against must report {INITIAL_HEALTH}, got {entry:?}"
            );
        }

        for metric_name in host_counts {
            let case = format!("{metric_name} with {registered} registered instances");
            let (status, body) = read_metrics(
                &emulator,
                DEFAULT_REGION,
                request(
                    name,
                    metric_name,
                    "Count",
                    300,
                    WINDOW_START,
                    WINDOW_START + 3_600,
                    &["Average", "Minimum", "Maximum"],
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            let data = metric_data(&case, &body);
            assert!(
                data.is_empty(),
                "case {case}: no health check ran, so the series must carry no datapoints, got \
                 {data:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reading changes nothing
// ---------------------------------------------------------------------------

/// RULE: reading metrics reports; it does not change what a later caller sees.
/// The same request answers the same thing twice, and the load balancer record is
/// exactly what it was before the reads.
#[tokio::test]
async fn reading_metrics_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let name = "quiet-lb";
    balancer(&emulator, DEFAULT_REGION, name).await;
    let before = record(&emulator, DEFAULT_REGION, name).await;

    let mut answers = Vec::new();
    for round in 0..2 {
        for (metric_name, unit) in PUBLISHED_UNITS.iter().take(4) {
            let case = format!("round {round} of {metric_name}");
            let (status, answer) = read_metrics(
                &emulator,
                DEFAULT_REGION,
                request(
                    name,
                    metric_name,
                    unit,
                    300,
                    WINDOW_START,
                    WINDOW_START + 3_600,
                    &["Average", "Sum"],
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {answer:?}"
            );
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

    let after = record(&emulator, DEFAULT_REGION, name).await;
    assert_eq!(
        before, after,
        "case the load balancer record after the reads: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals the handler owns
// ---------------------------------------------------------------------------

/// RULE: a request the model admits but that cannot describe a series — a metric
/// asked for in a unit it is not published in, a window whose end does not
/// follow its start, or a request naming no statistic — is refused with the
/// modelled 400 `InvalidInputException`.
#[tokio::test]
async fn a_request_that_describes_no_series_is_refused_as_invalid_input() {
    let emulator = Emulator::new();
    let name = "refusal-lb";
    balancer(&emulator, DEFAULT_REGION, name).await;
    let before = record(&emulator, DEFAULT_REGION, name).await;

    // One row per way the request fails to describe a series, crossed over the
    // units a metric is NOT published in (a unit of the wrong dimension, the unit
    // the other metrics of the same resource ARE published in, and one no load
    // balancer metric uses at all).
    let rows: &[(&str, Value)] = &[
        (
            "RequestCount asked for in Seconds, the unit of the response time",
            request(
                name,
                "RequestCount",
                "Seconds",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "InstanceResponseTime asked for in Count",
            request(
                name,
                "InstanceResponseTime",
                "Count",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Average"],
            ),
        ),
        (
            "HealthyHostCount asked for in Percent",
            request(
                name,
                "HealthyHostCount",
                "Percent",
                MIN_PERIOD,
                WINDOW_START,
                WINDOW_START + 600,
                &["Average"],
            ),
        ),
        (
            "ClientTLSNegotiationErrorCount asked for in None",
            request(
                name,
                "ClientTLSNegotiationErrorCount",
                "None",
                MAX_PERIOD,
                WINDOW_START,
                WINDOW_START + MAX_PERIOD,
                &["Sum"],
            ),
        ),
        (
            "HTTPCode_LB_5XX_Count asked for in Bytes",
            request(
                name,
                "HTTPCode_LB_5XX_Count",
                "Bytes",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "a window that ends where it starts",
            request(
                name,
                "RequestCount",
                "Count",
                300,
                WINDOW_START,
                WINDOW_START,
                &["Sum"],
            ),
        ),
        (
            "a window that ends before it starts",
            request(
                name,
                "UnhealthyHostCount",
                "Count",
                MAX_PERIOD,
                WINDOW_START + 3_600,
                WINDOW_START,
                &["Average"],
            ),
        ),
        (
            "a request naming no statistic",
            request(
                name,
                "HTTPCode_Instance_2XX_Count",
                "Count",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &[],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = read_metrics(&emulator, DEFAULT_REGION, body.clone()).await;
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
            answered["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case}: expected a message saying what was wrong, got {answered:?}"
        );
    }

    // A refused read is still a read: it changed nothing.
    let after = record(&emulator, DEFAULT_REGION, name).await;
    assert_eq!(
        before, after,
        "case the load balancer record after every refusal: expected {before:?}, got {after:?}"
    );
}

/// RULE: a name no load balancer record of the Region the request is made in
/// answers to — never created, created in another Region, or created and since
/// deleted — earns the service's `DoesNotExist` envelope: HTTP 400, the wire
/// error `NotFoundException`, the body `code` `DoesNotExist`, and a message
/// naming the resource type and the requested name.
#[tokio::test]
async fn a_name_no_load_balancer_answers_to_earns_the_does_not_exist_envelope() {
    let emulator = Emulator::new();

    // A load balancer that existed and was deleted, and one that exists in
    // ANOTHER Region than the read is made in.
    let deleted = "short-lived-lb";
    balancer(&emulator, DEFAULT_REGION, deleted).await;
    emulator
        .ok_at(
            None,
            Some(DEFAULT_REGION),
            "DeleteLoadBalancer",
            json!({"loadBalancerName": deleted}),
        )
        .await;
    let elsewhere = "other-region-lb";
    balancer(&emulator, "us-west-2", elsewhere).await;

    // Names differing along what the rule generalises over — length, case, digits
    // and separators — plus the deleted one and the one of another Region.
    let names = [
        "ab",
        "never-created",
        "Probe_7",
        "lb-0123456789",
        deleted,
        elsewhere,
    ];
    for name in names {
        let case = format!("GetLoadBalancerMetricData on {name:?} in {DEFAULT_REGION}");
        let (status, body) = read_metrics(
            &emulator,
            DEFAULT_REGION,
            request(
                name,
                "HealthyHostCount",
                "Count",
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
            Some(format!("The LoadBalancer does not exist: {name}").as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }

    // The load balancer of the other Region is served THERE, through the same
    // request body the read above was refused for.
    let case = format!("GetLoadBalancerMetricData on {elsewhere:?} in us-west-2");
    let (status, body) = read_metrics(
        &emulator,
        "us-west-2",
        request(
            elsewhere,
            "HealthyHostCount",
            "Count",
            300,
            WINDOW_START,
            WINDOW_START + 3_600,
            &["Average"],
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200, got {status} {body:?}"
    );
    assert!(
        metric_data(&case, &body).is_empty(),
        "case {case}: expected an empty series, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals the modelled constraints own
// ---------------------------------------------------------------------------

/// RULE: a value outside a modelled constraint is refused by the boundary walk
/// with its own 400 `ValidationException`, before the handler runs — the
/// `MetricPeriod` range one past each published bound, and a name outside each
/// enum this input reaches.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_by_the_boundary_walk() {
    let emulator = Emulator::new();
    let name = "boundary-lb";
    balancer(&emulator, DEFAULT_REGION, name).await;

    let rows: &[(&str, Value)] = &[
        (
            "a period one second below the range",
            request(
                name,
                "RequestCount",
                "Count",
                MIN_PERIOD - 1,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "a period one second above the range",
            request(
                name,
                "RequestCount",
                "Count",
                MAX_PERIOD + 1,
                WINDOW_START,
                WINDOW_START + 90_000,
                &["Sum"],
            ),
        ),
        (
            "a metric name outside the modelled enum",
            request(
                name,
                "TeapotTemperature",
                "Count",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "an instance metric, which a load balancer does not publish",
            request(
                name,
                "CPUUtilization",
                "Percent",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Average"],
            ),
        ),
        (
            "a unit outside the modelled enum",
            request(
                name,
                "RequestCount",
                "Furlongs",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "a statistic outside the modelled enum",
            request(
                name,
                "RequestCount",
                "Count",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Median"],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = read_metrics(&emulator, DEFAULT_REGION, body.clone()).await;
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
        let (status, body) = read_metrics(
            &emulator,
            DEFAULT_REGION,
            request(
                name,
                "RequestCount",
                "Count",
                period,
                WINDOW_START,
                WINDOW_START + period,
                &["Sum"],
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert!(
            metric_data(&case, &body).is_empty(),
            "case {case}: expected an empty series, got {body:?}"
        );
    }
}
