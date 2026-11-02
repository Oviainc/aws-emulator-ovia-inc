// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRDSDatabaseRecommendationProjectedMetrics`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /` through
//! the same `MultiServiceRouter` the emulator binary mounts — so a response that is not
//! computed from persisted state fails here.
//!
//! Compute Optimizer's own analysis of an observed workload is what records a database row,
//! and no operation of THIS API creates one, so the fixtures below record rows into the
//! findings catalog the way that analysis would and then read them back only over the wire.
//! What is asserted is the RULE the read follows, over a table of inputs rather than one
//! recorded call:
//!
//! * the series spans the requested window at the requested granularity — one data point per
//!   period that begins inside it;
//! * each metric is projected onto the recommended DB instance class: a metric measured as a
//!   percentage of what the instance class sizes (its vCPU, its memory) scales by the ratio
//!   of the two classes' capacities and cannot pass 100, and every other metric — the
//!   workload's own and the storage's — is projected unchanged;
//! * `stat` selects which observed statistic is projected;
//! * a database the analysis would not move has no option, so there is nothing to project.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RDSDBUtilizationMetric;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY,
    MAX_PROJECTED_METRIC_DATA_POINTS, SOURCE_TYPE_RDS_DB_INSTANCE,
};

use crate::harness::{call, call_in_region, emulator_with_engine, error_code, opt_in, ACCOUNT};

/// The region the harness signs for, and so the regional endpoint every call below addresses
/// unless it names another.
const REGION: &str = "us-east-1";

/// The account the emulator's own findings catalog is seeded for, and the database it holds.
const SEEDED_ACCOUNT: &str = "000000000000";

/// The first data point every window below asks for, in epoch seconds.
const WINDOW_START: i64 = 1_761_000_000;

/// A DB instance ARN: "arn:aws:rds:{region}:{accountId}:db:{resourceName}"
/// (`GetRDSDatabaseRecommendationProjectedMetrics$resourceArn`).
fn db_arn(account_id: &str, name: &str) -> String {
    format!("arn:aws:rds:{REGION}:{account_id}:db:{name}")
}

/// One utilization metric as the analysis observed it: an `RDSDBMetricName`, the
/// `RDSDBMetricStatistic` it was observed under, and its value.
fn metric(name: &str, statistic: &str, value: f64) -> RDSDBUtilizationMetric {
    RDSDBUtilizationMetric {
        name: Some(name.to_string()),
        statistic: Some(statistic.to_string()),
        value,
    }
}

/// What the analysis recorded about one database: the class it runs on, the class the
/// analysis would move it to (`None` where it would keep the current one) and the metrics
/// observed on it.
fn detail(
    current: &str,
    recommended: Option<&str>,
    metrics: Vec<RDSDBUtilizationMetric>,
) -> Option<RdsDatabaseDetail> {
    Some(RdsDatabaseDetail {
        current_db_instance_class: Some(current.to_string()),
        recommended_db_instance_class: recommended.map(ToString::to_string),
        utilization_metrics: metrics,
        ..RdsDatabaseDetail::default()
    })
}

/// One analysed resource as the findings catalog records it, for a resource analysed as an
/// Amazon Aurora or RDS database. The catalog records the resource's region-free identity:
/// the ARN reported for it names the region of the endpoint that reports it.
fn database(
    name: &str,
    finding: &str,
    current_monthly_cost: f64,
    recommended_monthly_cost: f64,
    rds_database: Option<RdsDatabaseDetail>,
) -> AnalysedResource {
    AnalysedResource {
        account_id: ACCOUNT.to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        arn_service: "rds".to_string(),
        arn_resource: format!("db:{name}"),
        finding: finding.to_string(),
        finding_reason_codes: Vec::new(),
        current_performance_risk: "Low".to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        inferred_workload_types: Vec::new(),
        rds_database,
    }
}

/// The analysis output every fixture below reads: databases whose recommended class is
/// smaller, larger, far smaller, far larger and unknown to the size catalog, one the analysis
/// would not move at all, and one whose finding is about its storage rather than its
/// instance.
fn corpus() -> Vec<AnalysedResource> {
    vec![
        // Half the capacity: every percentage of the instance's own resources doubles. Its
        // observations span three statistics, so `stat` has something to select.
        database(
            "orders-writer",
            "Overprovisioned",
            200.0,
            150.0,
            detail(
                "db.r6g.2xlarge",
                Some("db.r6g.xlarge"),
                vec![
                    metric("CPU", "Maximum", 17.5),
                    metric("Memory", "Maximum", 24.0),
                    metric("DatabaseConnections", "Maximum", 12.0),
                    metric("CPU", "Average", 8.0),
                    metric("EBSVolumeStorageSpaceUtilization", "Average", 40.0),
                    // `RDSDBMetricStatistic` declares `Minimum`, which no `MetricStatistic`
                    // a request can ask for names.
                    metric("Memory", "Minimum", 5.0),
                ],
            ),
        ),
        // Twice the capacity, across the named sizes below `xlarge`: the percentages halve.
        database(
            "reports-primary",
            "Underprovisioned",
            100.0,
            180.0,
            detail(
                "db.t4g.medium",
                Some("db.t4g.large"),
                vec![
                    metric("CPU", "Maximum", 90.0),
                    metric("Memory", "Maximum", 88.0),
                ],
            ),
        ),
        // Sixteen times smaller: the percentages would pass 100, which no utilization
        // percentage can, while the connection count — which the instance class does not
        // size — keeps its own value above 100.
        database(
            "hot-shard",
            "Overprovisioned",
            500.0,
            300.0,
            detail(
                "db.r6g.8xlarge",
                Some("db.r6g.large"),
                vec![
                    metric("CPU", "Maximum", 10.0),
                    metric("DatabaseConnections", "Maximum", 300.0),
                ],
            ),
        ),
        // The smallest named sizes: `micro` to `small` is twice the capacity.
        database(
            "tiny-shard",
            "Overprovisioned",
            20.0,
            10.0,
            detail(
                "db.t3.micro",
                Some("db.t3.small"),
                vec![
                    metric("CPU", "Maximum", 40.0),
                    metric("Memory", "Average", 60.0),
                ],
            ),
        ),
        // Large `<N>xlarge` sizes: 24 to 12 is half the capacity.
        database(
            "wide-shard",
            "Overprovisioned",
            900.0,
            450.0,
            detail(
                "db.r5.24xlarge",
                Some("db.r5.12xlarge"),
                vec![metric("CPU", "Maximum", 25.0)],
            ),
        ),
        // A size name the catalog records no capacity for: there is no ratio to project by,
        // so the observation is projected unchanged.
        database(
            "legacy-metal",
            "Overprovisioned",
            80.0,
            60.0,
            detail(
                "db.m5.metal",
                Some("db.m5.24xlarge"),
                vec![metric("CPU", "Maximum", 30.0)],
            ),
        ),
        // Optimized, and the analysis recorded no database configuration at all: nothing to
        // move, so no option.
        database("audit-archive", "Optimized", 90.0, 90.0, None),
        // The finding is about the STORAGE, so the instance side is optimized: this row has
        // a storage option and no instance one.
        database(
            "ledger-primary",
            "NotOptimized",
            60.0,
            40.0,
            detail(
                "db.m6g.large",
                None,
                vec![metric("CPU", "Maximum", 20.0)],
            ),
        ),
    ]
}

/// Record `resources` into the findings catalog this reader reports from, the way Compute
/// Optimizer's analysis would.
fn record(engine: &Arc<engine::InMemoryEngine>, resources: &[AnalysedResource]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for resource in resources {
        guard.analysed_resources.push(resource.clone());
    }
}

/// How many rows the findings catalog holds.
fn catalog_size(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the engine bears this service's state")
        .lock()
        .analysed_resources
        .len()
}

/// An emulator whose account is opted in, whose analysis is due and whose findings catalog
/// holds [`corpus`].
///
/// A projection is reported only once the analysis the enrollment started is due
/// ([`ANALYSIS_DELAY`]), so the fixture waits that span out before reading, the way every
/// other recommendation suite of this crate does; the extra margin covers scheduler jitter.
async fn emulator_with_corpus() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
) {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    record(&engine, &corpus());
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    (router, engine)
}

/// A request for one database over one window.
fn request(name: &str, stat: &str, period: i64, start: i64, end: i64) -> Value {
    json!({
        "resourceArn": db_arn(ACCOUNT, name),
        "stat": stat,
        "period": period,
        "startTime": start,
        "endTime": end,
    })
}

/// Read, asserting the call was accepted.
async fn projected(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    body: Value,
) -> Value {
    let (status, response) = call(
        router,
        "GetRDSDatabaseRecommendationProjectedMetrics",
        body.clone(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {body}, got {status}: {response}"
    );
    response
}

/// The one recommendation option a response reports, asserting there is exactly one.
fn only_option(case: &str, response: &Value) -> Value {
    let options = response["recommendedOptionProjectedMetrics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {response}"));
    assert_eq!(
        options.len(),
        1,
        "case {case:?}: the analysis records one recommended option, got {response}"
    );
    options[0].clone()
}

/// The projected series of one metric name, or `None` when the option reports none.
fn series(option: &Value, name: &str) -> Option<Value> {
    option["projectedMetrics"]
        .as_array()?
        .iter()
        .find(|metric| metric["name"] == json!(name))
        .cloned()
}

/// The metric names an option reports, in the order reported.
fn reported_names(option: &Value) -> Vec<String> {
    option["projectedMetrics"]
        .as_array()
        .map(|metrics| {
            metrics
                .iter()
                .map(|metric| metric["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Two metric values agree when they agree to the hundredth the service rounds to.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-6
}

/// The series reported over `[start, end)` at a `period`-second granularity carries one data
/// point per period that BEGINS inside the window, the first at `startTime`, and one value
/// per timestamp. Each row differs along the granularity, the window length, and whether the
/// window divides into whole periods.
#[tokio::test]
async fn the_series_spans_the_requested_window_at_the_requested_granularity() {
    let (router, _engine) = emulator_with_corpus().await;

    // (case, period in seconds, window length in seconds, the data points expected)
    let rows: Vec<(&str, i64, i64, usize)> = vec![
        ("one hour at an hourly granularity", 3_600, 3_600, 1),
        ("four hours of hourly points", 3_600, 4 * 3_600, 4),
        ("an hour of five-minute points", 300, 3_600, 12),
        ("a window shorter than one period", 600, 599, 1),
        ("a window that is not whole periods", 60, 90, 2),
        ("a fortnight of daily points", 86_400, 14 * 86_400, 14),
        (
            "the most data points one response carries",
            60,
            60 * MAX_PROJECTED_METRIC_DATA_POINTS as i64,
            MAX_PROJECTED_METRIC_DATA_POINTS,
        ),
    ];

    for (case, period, length, points) in rows {
        let response = projected(
            &router,
            case,
            request(
                "orders-writer",
                "Maximum",
                period,
                WINDOW_START,
                WINDOW_START + length,
            ),
        )
        .await;
        let option = only_option(case, &response);
        let cpu = series(&option, "CPU")
            .unwrap_or_else(|| panic!("case {case:?}: a projected CPU series, got {option}"));
        let timestamps = cpu["timestamps"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a list of timestamps, got {cpu}"));
        assert_eq!(
            timestamps.len(),
            points,
            "case {case:?}: expected {points} data points, got {}",
            timestamps.len()
        );
        let expected: Vec<f64> = (0..points)
            .map(|index| (WINDOW_START + period * index as i64) as f64)
            .collect();
        let reported: Vec<f64> = timestamps
            .iter()
            .map(|at| at.as_f64().unwrap_or(f64::NAN))
            .collect();
        assert!(
            reported
                .iter()
                .zip(&expected)
                .all(|(actual, wanted)| close(*actual, *wanted)),
            "case {case:?}: expected data points every {period}s from {WINDOW_START}, \
             got {reported:?}"
        );
        assert!(
            reported.last() < Some(&((WINDOW_START + length) as f64)),
            "case {case:?}: every data point begins inside the window, got {reported:?}"
        );
        let values = cpu["values"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a list of values, got {cpu}"));
        assert_eq!(
            values.len(),
            timestamps.len(),
            "case {case:?}: one value per timestamp, got {cpu}"
        );
        // The analysis recorded ONE figure per metric and statistic, so every data point of
        // the window carries the same projected value.
        assert!(
            values
                .iter()
                .all(|value| close(value.as_f64().unwrap_or(f64::NAN), 35.0)),
            "case {case:?}: 17.5% of twice the capacity is 35% of half of it, got {cpu}"
        );
    }
}

/// One row of the projection table: the case, the database, the statistic asked for, the
/// class the option is expected to name, and the series expected — each metric name with the
/// value it carries at every data point.
type ProjectionRow<'a> = (&'a str, &'a str, &'a str, &'a str, Vec<(&'a str, f64)>);

/// The projection rule: a metric measured as a percentage of a resource the DB instance class
/// sizes scales by the ratio of the two classes' capacities and cannot pass the percentage
/// ceiling, and every other metric — the workload's own, the storage's — is projected
/// unchanged. Each row differs along the direction and the size of the capacity change.
#[tokio::test]
async fn each_metric_is_projected_onto_the_recommended_class() {
    let (router, _engine) = emulator_with_corpus().await;

    // (case, database, stat, recommendedDBInstanceClass, the series expected)
    let rows: Vec<ProjectionRow> = vec![
        (
            "half the capacity doubles each percentage and leaves the workload alone",
            "orders-writer",
            "Maximum",
            "db.r6g.xlarge",
            vec![
                ("CPU", 35.0),
                ("Memory", 48.0),
                ("DatabaseConnections", 12.0),
            ],
        ),
        (
            "the same database under the other statistic",
            "orders-writer",
            "Average",
            "db.r6g.xlarge",
            // The storage's own percentage is projected unchanged: an instance-class change
            // does not resize the storage.
            vec![("CPU", 16.0), ("EBSVolumeStorageSpaceUtilization", 40.0)],
        ),
        (
            "twice the capacity halves each percentage",
            "reports-primary",
            "Maximum",
            "db.t4g.large",
            vec![("CPU", 45.0), ("Memory", 44.0)],
        ),
        (
            "a projection that would pass the ceiling stops at it",
            "hot-shard",
            "Maximum",
            "db.r6g.large",
            // 10% of sixteen times the capacity is 160% of a sixteenth of it.
            vec![("CPU", 100.0), ("DatabaseConnections", 300.0)],
        ),
        (
            "the smallest named sizes",
            "tiny-shard",
            "Maximum",
            "db.t3.small",
            vec![("CPU", 20.0)],
        ),
        (
            "the smallest named sizes, under the other statistic",
            "tiny-shard",
            "Average",
            "db.t3.small",
            vec![("Memory", 30.0)],
        ),
        (
            "the largest named sizes",
            "wide-shard",
            "Maximum",
            "db.r5.12xlarge",
            vec![("CPU", 50.0)],
        ),
        (
            "a size name no capacity is recorded for projects nothing",
            "legacy-metal",
            "Maximum",
            "db.m5.24xlarge",
            vec![("CPU", 30.0)],
        ),
    ];

    for (case, name, stat, recommended_class, expected) in rows {
        let response = projected(
            &router,
            case,
            request(name, stat, 3_600, WINDOW_START, WINDOW_START + 2 * 3_600),
        )
        .await;
        let option = only_option(case, &response);
        assert_eq!(
            option["recommendedDBInstanceClass"],
            json!(recommended_class),
            "case {case:?}: the option names the class the analysis recorded, got {option}"
        );
        assert_eq!(
            option["rank"],
            json!(1),
            "case {case:?}: the one recorded option is the first-ranked one, got {option}"
        );
        let wanted_names: Vec<String> = expected
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        assert_eq!(
            reported_names(&option),
            wanted_names,
            "case {case:?}: expected exactly the metrics observed under {stat:?}, got {option}"
        );
        for (metric_name, wanted) in expected {
            let reported = series(&option, metric_name).unwrap_or_else(|| {
                panic!("case {case:?}: a projected {metric_name} series, got {option}")
            });
            let values: Vec<f64> = reported["values"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a list of values, got {reported}"))
                .iter()
                .map(|value| value.as_f64().unwrap_or(f64::NAN))
                .collect();
            assert_eq!(
                values.len(),
                2,
                "case {case:?}: two hourly data points were asked for, got {reported}"
            );
            assert!(
                values.iter().all(|value| close(*value, wanted)),
                "case {case:?}: metric {metric_name:?}: expected {wanted} at every data \
                 point, got {values:?}"
            );
        }
    }
}

/// `stat` is "The statistic of the projected metrics": an observation the analysis recorded
/// under another statistic is not reported under the requested one — including the statistic
/// `RDSDBMetricStatistic` declares that no request can ask for.
#[tokio::test]
async fn the_requested_statistic_selects_the_observations_projected() {
    let (router, _engine) = emulator_with_corpus().await;

    for (case, stat, wanted) in [
        (
            "the maximum observations",
            "Maximum",
            vec!["CPU", "Memory", "DatabaseConnections"],
        ),
        (
            "the average observations",
            "Average",
            vec!["CPU", "EBSVolumeStorageSpaceUtilization"],
        ),
    ] {
        let response = projected(
            &router,
            case,
            request(
                "orders-writer",
                stat,
                3_600,
                WINDOW_START,
                WINDOW_START + 3_600,
            ),
        )
        .await;
        let option = only_option(case, &response);
        assert_eq!(
            reported_names(&option),
            wanted
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the metrics observed under {stat:?}, got {option}"
        );
    }

    // `Memory` was observed under `Minimum` too, and that observation is reported under
    // neither statistic a request can ask for: 5% of twice the capacity would read 10%.
    for stat in ["Maximum", "Average"] {
        let response = projected(
            &router,
            "a Minimum observation",
            request(
                "orders-writer",
                stat,
                3_600,
                WINDOW_START,
                WINDOW_START + 3_600,
            ),
        )
        .await;
        let option = only_option("a Minimum observation", &response);
        let memory = series(&option, "Memory")
            .and_then(|metric| metric["values"][0].as_f64())
            .unwrap_or(f64::NAN);
        assert!(
            !close(memory, 10.0),
            "stat {stat:?}: a Minimum observation is projected under no requested \
             statistic, got {option}"
        );
    }
}

/// A database whose instance configuration the analysis would not change has no recommended
/// option, so there is nothing to project metrics onto — and an option the analysis recorded
/// no observation for under the requested statistic reports no series rather than an empty
/// one.
#[tokio::test]
async fn a_database_the_analysis_would_not_move_reports_no_option() {
    let (router, _engine) = emulator_with_corpus().await;

    for (case, name) in [
        ("optimized, and no configuration recorded", "audit-archive"),
        ("the finding is about the storage", "ledger-primary"),
    ] {
        let response = projected(
            &router,
            case,
            request(name, "Maximum", 3_600, WINDOW_START, WINDOW_START + 3_600),
        )
        .await;
        assert_eq!(
            response["recommendedOptionProjectedMetrics"],
            json!([]),
            "case {case:?}: no instance option, so nothing to project, got {response}"
        );
    }

    // The same database, read under a statistic the analysis observed nothing under: the
    // option is still the recorded one, and it carries no series.
    let case = "no observation under the requested statistic";
    let response = projected(
        &router,
        case,
        request(
            "wide-shard",
            "Average",
            3_600,
            WINDOW_START,
            WINDOW_START + 3_600,
        ),
    )
    .await;
    let option = only_option(case, &response);
    assert_eq!(
        option["recommendedDBInstanceClass"],
        json!("db.r5.12xlarge"),
        "case {case:?}: the option is the one the analysis recorded, got {option}"
    );
    assert!(
        option["projectedMetrics"].is_null(),
        "case {case:?}: nothing was observed under Average, so no series is reported, \
         got {option}"
    );
}

/// A request that is not a series — a granularity that is not a positive span, a window that
/// does not run forwards — or one that would carry more data points than a response can, is
/// refused with the parameter named; a resource this endpoint holds no database analysis for
/// is reported missing.
#[tokio::test]
async fn a_request_that_is_not_a_series_is_refused() {
    let (router, _engine) = emulator_with_corpus().await;
    let window = (WINDOW_START, WINDOW_START + 3_600);

    // (case, request, the status expected, the error code expected)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "the modelled default granularity divides the window into nothing",
            request("orders-writer", "Maximum", 0, window.0, window.1),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a granularity that is not a span of time",
            request("orders-writer", "Maximum", -60, window.0, window.1),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an empty window",
            request("orders-writer", "Maximum", 3_600, window.0, window.0),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a window that runs backwards",
            request("orders-writer", "Maximum", 3_600, window.1, window.0),
            400,
            "InvalidParameterValueException",
        ),
        (
            "one data point past the most a response carries",
            request(
                "orders-writer",
                "Maximum",
                60,
                WINDOW_START,
                WINDOW_START + 60 * (MAX_PROJECTED_METRIC_DATA_POINTS as i64 + 1),
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a resource that is not an ARN",
            json!({
                "resourceArn": "orders-writer",
                "stat": "Maximum",
                "period": 3_600,
                "startTime": window.0,
                "endTime": window.1,
            }),
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (case, body, status_wanted, code) in rows {
        let (status, response) = call(
            &router,
            "GetRDSDatabaseRecommendationProjectedMetrics",
            body.clone(),
        )
        .await;
        assert_eq!(
            status, status_wanted,
            "case {case:?}: {body} must be refused with {status_wanted}, got {status}: \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {case:?}: expected {code}, got {response}"
        );
    }

    // A resource this service holds no database analysis of has no projection to report,
    // which is a SUCCESSFUL empty list rather than a failure: a caller charting the
    // projection of each database renders an empty series instead of aborting.
    for (case, body) in [
        (
            "a database this service holds no analysis of",
            request("no-such-database", "Maximum", 3_600, window.0, window.1),
        ),
        (
            "an ARN that names no database at all",
            json!({
                "resourceArn": format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789"),
                "stat": "Maximum",
                "period": 3_600,
                "startTime": window.0,
                "endTime": window.1,
            }),
        ),
    ] {
        let response = projected(&router, case, body).await;
        assert_eq!(
            response["recommendedOptionProjectedMetrics"],
            json!([]),
            "case {case:?}: expected an empty list of projections, got {response}"
        );
    }

    // The ceiling itself is served: the bound is on the response, not on the request's shape.
    let case = "the ceiling itself";
    let response = projected(
        &router,
        case,
        request(
            "orders-writer",
            "Maximum",
            60,
            WINDOW_START,
            WINDOW_START + 60 * MAX_PROJECTED_METRIC_DATA_POINTS as i64,
        ),
    )
    .await;
    let option = only_option(case, &response);
    assert_eq!(
        series(&option, "CPU")
            .and_then(|metric| metric["timestamps"].as_array().map(Vec::len))
            .unwrap_or_default(),
        MAX_PROJECTED_METRIC_DATA_POINTS,
        "case {case:?}: the ceiling is served in full, got {option}"
    );
}

/// Compute Optimizer is a REGIONAL service: the endpoint a request addresses is the region
/// whose resources it reports on, so a database named by the ARN of another region is one
/// this endpoint holds no recommendation for — answered with the empty list of projections.
#[tokio::test]
async fn a_database_of_another_region_is_not_this_endpoints() {
    let (router, _engine) = emulator_with_corpus().await;
    let body = request(
        "orders-writer",
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );

    // (case, the region the read is made at, whether that endpoint holds the analysis)
    for (case, region, analysed_here) in [
        ("the region the row was read in", REGION, true),
        ("another commercial region", "eu-west-1", false),
        ("a region on another continent", "ap-southeast-2", false),
    ] {
        let (status, response) = call_in_region(
            &router,
            region,
            "GetRDSDatabaseRecommendationProjectedMetrics",
            body.clone(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: at {region}, expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["recommendedOptionProjectedMetrics"] != json!([]),
            analysed_here,
            "case {case:?}: at {region}, the projection is reported exactly where the \
             analysis is held, got {response}"
        );
    }
}

/// The service analyses an account's resources only once it has opted in, and only once the
/// analysis that enrollment starts is due: an account that never opted in reads no projection
/// at all, and one that has just opted in reads no analysis yet.
#[tokio::test]
async fn the_read_waits_for_the_enrollment_and_the_analysis() {
    let (router, engine) = emulator_with_engine();
    record(&engine, &corpus());

    let body = request(
        "orders-writer",
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );
    let (status, response) = call(
        &router,
        "GetRDSDatabaseRecommendationProjectedMetrics",
        body.clone(),
    )
    .await;
    assert_eq!(
        status, 403,
        "before opting in: expected 403, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "OptInRequiredException",
        "before opting in: expected OptInRequiredException, got {response}"
    );

    opt_in(&router).await;
    let response = projected(&router, "just opted in", body.clone()).await;
    assert_eq!(
        response["recommendedOptionProjectedMetrics"],
        json!([]),
        "just opted in: the analysis is not due yet, so there is nothing to project, got \
         {response}"
    );

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let response = projected(&router, "once the analysis is due", body).await;
    assert!(
        !response["recommendedOptionProjectedMetrics"]
            .as_array()
            .expect("a list of options")
            .is_empty(),
        "once the analysis is due the projection is served, got {response}"
    );
}

/// The projection and the recommendation are two readers of ONE catalog row: the option this
/// operation projects onto is the option `GetRDSDatabaseRecommendations` reports, under the
/// same class and the same rank. A read reports: it must not change what a later caller sees.
#[tokio::test]
async fn the_projected_option_is_the_recommended_option() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    // The catalog is read only once the analysis the enrollment started is due.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let before = catalog_size(&engine);
    let seeded = db_arn(SEEDED_ACCOUNT, "orders-primary");

    let (status, recommendations) = call(
        &router,
        "GetRDSDatabaseRecommendations",
        json!({"resourceArns": [seeded.clone()]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the recommendation of the seeded database: got {status}: {recommendations}"
    );
    let recommended = &recommendations["rdsDBRecommendations"][0]["instanceRecommendationOptions"]
        [0]["dbInstanceClass"];

    let body = json!({
        "resourceArn": seeded,
        "stat": "Maximum",
        "period": 3_600,
        "startTime": WINDOW_START,
        "endTime": WINDOW_START + 3 * 3_600,
    });
    let response = projected(&router, "the seeded database", body.clone()).await;
    let option = only_option("the seeded database", &response);
    assert_eq!(
        &option["recommendedDBInstanceClass"], recommended,
        "the projection is onto the recommended option: {option} vs {recommendations}"
    );
    assert_eq!(
        option["rank"], recommendations["rdsDBRecommendations"][0]["instanceRecommendationOptions"][0]["rank"],
        "both readers report the one option at the same rank: {option} vs {recommendations}"
    );
    // Stored: 17.5% CPU, 24% memory and 12 connections observed on db.r6g.2xlarge, with
    // db.r6g.xlarge recommended — half the capacity.
    for (name, wanted) in [("CPU", 35.0), ("Memory", 48.0), ("DatabaseConnections", 12.0)] {
        let reported = series(&option, name)
            .unwrap_or_else(|| panic!("a projected {name} series, got {option}"));
        let values: Vec<f64> = reported["values"]
            .as_array()
            .expect("a list of values")
            .iter()
            .map(|value| value.as_f64().unwrap_or(f64::NAN))
            .collect();
        assert_eq!(values.len(), 3, "three hourly data points, got {reported}");
        assert!(
            values.iter().all(|value| close(*value, wanted)),
            "metric {name:?}: expected {wanted} at every data point, got {values:?}"
        );
    }

    let again = projected(&router, "the same read again", body).await;
    assert_eq!(
        again, response,
        "a second read must report the same projection, got {again}"
    );
    assert_eq!(
        catalog_size(&engine),
        before,
        "a read must not add or drop a catalog row"
    );
}
