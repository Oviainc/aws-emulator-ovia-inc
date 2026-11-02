// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetEC2RecommendationProjectedMetrics`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /` through
//! the same `MultiServiceRouter` the emulator binary mounts — so a response that is not
//! computed from persisted state fails here.
//!
//! Compute Optimizer's own analysis of an observed workload is what records an instance, and
//! no operation of THIS API creates one, so the fixtures below record instances into the
//! analysis the way that analysis would and then read them back only over the wire. What is
//! asserted is the RULE the read follows, over a table of inputs rather than one recorded
//! call:
//!
//! * the series spans the requested window at the requested granularity — one data point per
//!   period that begins inside it;
//! * each metric is projected onto each recommended instance type: a metric measured as a
//!   percentage of a resource the instance type sizes (its vCPUs, its memory) scales by the
//!   ratio of the two types' capacities for THAT resource and cannot pass 100, and those two
//!   are the only metrics this action reports a series for — an observation of the workload's
//!   own traffic is projected by no option;
//! * `stat` selects which observed statistic is projected, and the CPU vendor architecture
//!   preference selects which candidates are projected at all;
//! * an instance the analysis would not move has no option, so there is nothing to project;
//! * an instance this endpoint holds no analysis of has no projection either, which is a
//!   successful empty list rather than a failure.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, InstanceConfiguration, ObservedInstance, ObservedInstanceMetric,
    ANALYSIS_DELAY, CPU_ARCHITECTURE_AWS_ARM64, CPU_ARCHITECTURE_CURRENT, INSTANCE_STATE_RUNNING,
    MAX_PROJECTED_METRIC_DATA_POINTS, PERFORMANCE_RISK_VERY_LOW,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in_region, emulator_with_engine, error_code, opt_in, ACCOUNT};

/// The region the harness signs for, and so the regional endpoint every call below addresses
/// unless it names another.
const REGION: &str = "us-east-1";

/// The first data point every window below asks for, in epoch seconds.
const WINDOW_START: i64 = 1_761_000_000;

/// An EC2 instance ARN in `region`: `arn:aws:ec2:<region>:<account>:instance/<id>`
/// (`GetEC2RecommendationProjectedMetrics$instanceArn`).
fn instance_arn(region: &str, account_id: &str, instance_id: &str) -> String {
    format!("arn:aws:ec2:{region}:{account_id}:instance/{instance_id}")
}

/// One instance type as the analysis records it: the type, the vCPUs and memory (GiB) it
/// provides, its on-demand price per hour and its `CpuVendorArchitecture`.
fn configuration(
    instance_type: &str,
    vcpus: f64,
    memory_gib: f64,
    hourly_price: f64,
    architecture: &str,
) -> InstanceConfiguration {
    InstanceConfiguration {
        instance_type: instance_type.to_string(),
        vcpus,
        memory_gib,
        hourly_price,
        architecture: architecture.to_string(),
    }
}

/// One utilization figure as the analysis observed it: a `MetricName`, the `MetricStatistic`
/// it was observed under, and its value.
fn metric(name: &str, statistic: &str, value: f64) -> ObservedInstanceMetric {
    ObservedInstanceMetric {
        name: name.to_string(),
        statistic: statistic.to_string(),
        value,
    }
}

/// One analysed instance, as Compute Optimizer's analysis of an observed workload records it.
fn instance(
    instance_id: &str,
    current: InstanceConfiguration,
    candidates: Vec<InstanceConfiguration>,
    utilization: Vec<ObservedInstanceMetric>,
) -> ObservedInstance {
    ObservedInstance {
        instance_id: instance_id.to_string(),
        account_id: ACCOUNT.to_string(),
        current,
        candidates,
        utilization,
        finding_reason_codes: Vec::new(),
        current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
        inferred_workload_types: Vec::new(),
        instance_state: INSTANCE_STATE_RUNNING.to_string(),
        tags: Vec::new(),
    }
}

/// The ids of the analysed instances [`corpus`] records.
const SIZED_DOWN: &str = "i-0b1b2c3d4e5f70001";
const MEMORY_ONLY: &str = "i-0b1b2c3d4e5f70002";
const FAR_SMALLER: &str = "i-0b1b2c3d4e5f70003";
const SAME_CAPACITY: &str = "i-0b1b2c3d4e5f70004";
const NOT_MOVED: &str = "i-0b1b2c3d4e5f70005";
const TWO_CANDIDATES: &str = "i-0b1b2c3d4e5f70006";
const SIZED_UP: &str = "i-0b1b2c3d4e5f70007";

/// The analysis output every fixture below reads: instances whose recommended type is
/// smaller, larger, far smaller, smaller in ONE dimension only, of the same capacity and
/// unknown to the analysis altogether, plus one with two candidates of different CPU vendor
/// architectures.
///
/// The vCPU and memory figures are those of the named instance types in the Amazon EC2
/// instance-type documentation; the prices are their public on-demand Linux prices in
/// `us-east-1`.
fn corpus() -> Vec<ObservedInstance> {
    vec![
        // Half the vCPUs and half the memory: every percentage of the instance's own
        // resources doubles. Its observations span two statistics, so `stat` has something
        // to select, and one — the workload's own network traffic — is of neither resource
        // the instance type sizes, so this action reports no series for it.
        instance(
            SIZED_DOWN,
            configuration("c5.4xlarge", 16.0, 32.0, 0.68, CPU_ARCHITECTURE_CURRENT),
            vec![configuration(
                "c5.2xlarge",
                8.0,
                16.0,
                0.34,
                CPU_ARCHITECTURE_CURRENT,
            )],
            vec![
                metric("Cpu", "Maximum", 20.0),
                metric("Memory", "Maximum", 30.0),
                metric("NETWORK_IN_BYTES_PER_SECOND", "Maximum", 4_096.0),
                metric("Cpu", "Average", 11.0),
            ],
        ),
        // The same vCPUs with four times the memory: the CPU percentage is unchanged and the
        // memory percentage is quartered, because each metric scales by the capacity of the
        // resource IT measures.
        instance(
            MEMORY_ONLY,
            configuration("t3.small", 2.0, 2.0, 0.0208, CPU_ARCHITECTURE_CURRENT),
            vec![configuration(
                "t3.large",
                2.0,
                8.0,
                0.0832,
                CPU_ARCHITECTURE_CURRENT,
            )],
            vec![
                metric("Cpu", "Maximum", 46.0),
                metric("Memory", "Maximum", 96.0),
            ],
        ),
        // Far smaller: both projections would pass 100, which no utilization percentage can,
        // while the workload's own packet rate is of neither resource the instance type sizes.
        instance(
            FAR_SMALLER,
            configuration("r5.12xlarge", 48.0, 384.0, 3.024, CPU_ARCHITECTURE_CURRENT),
            vec![configuration(
                "r5.large",
                2.0,
                16.0,
                0.126,
                CPU_ARCHITECTURE_CURRENT,
            )],
            vec![
                metric("Cpu", "Maximum", 10.0),
                metric("Memory", "Maximum", 5.0),
                metric("NETWORK_PACKETS_IN_PER_SECOND", "Maximum", 2_048.0),
            ],
        ),
        // A newer generation at the same capacity: there is a recommendation, and every
        // percentage is projected unchanged.
        instance(
            SAME_CAPACITY,
            configuration("m5.xlarge", 4.0, 16.0, 0.192, CPU_ARCHITECTURE_CURRENT),
            vec![configuration(
                "m6i.xlarge",
                4.0,
                16.0,
                0.1728,
                CPU_ARCHITECTURE_CURRENT,
            )],
            vec![
                metric("Cpu", "Maximum", 62.5),
                metric("Memory", "Maximum", 71.0),
            ],
        ),
        // The analysis found nothing to move this instance to: no option, so nothing to
        // project onto.
        instance(
            NOT_MOVED,
            configuration("m5.large", 2.0, 8.0, 0.096, CPU_ARCHITECTURE_CURRENT),
            Vec::new(),
            vec![metric("Cpu", "Maximum", 58.0)],
        ),
        // Two candidates of the same capacity and different CPU vendor architectures, in the
        // analysis' own rank order.
        instance(
            TWO_CANDIDATES,
            configuration("m5.2xlarge", 8.0, 32.0, 0.384, CPU_ARCHITECTURE_CURRENT),
            vec![
                configuration("m5.xlarge", 4.0, 16.0, 0.192, CPU_ARCHITECTURE_CURRENT),
                configuration("m6g.xlarge", 4.0, 16.0, 0.154, CPU_ARCHITECTURE_AWS_ARM64),
            ],
            vec![
                metric("Cpu", "Maximum", 24.0),
                metric("Memory", "Maximum", 36.0),
            ],
        ),
        // Sized UP by three quarters of the capacity it has: a ratio that is neither a
        // doubling nor a halving.
        instance(
            SIZED_UP,
            configuration("c5.9xlarge", 36.0, 72.0, 1.53, CPU_ARCHITECTURE_CURRENT),
            vec![configuration(
                "c5.12xlarge",
                48.0,
                96.0,
                2.04,
                CPU_ARCHITECTURE_CURRENT,
            )],
            vec![
                metric("Cpu", "Maximum", 60.0),
                metric("Memory", "Maximum", 80.0),
            ],
        ),
    ]
}

/// Record `instances` into the analysis this reader reports from, the way Compute Optimizer's
/// own analysis of an observed workload would.
fn record(engine: &Arc<engine::InMemoryEngine>, instances: Vec<ObservedInstance>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for instance in instances {
        guard.instances.insert(instance.instance_id.clone(), instance);
    }
}

/// How many instances the analysis holds.
fn analysed_instances(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the engine bears this service's state")
        .lock()
        .instances
        .len()
}

/// Opt the account in and let the analysis its enrollment starts come due — the gate every
/// recommendation reader of this service applies. The span is the service's own
/// ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// An emulator whose account is opted in, whose analysis is due and which holds [`corpus`].
async fn emulator_with_corpus() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    record(&engine, corpus());
    analysed(&router).await;
    (router, engine)
}

/// A request for one instance over one window.
fn request(instance_id: &str, stat: &str, period: i64, start: i64, end: i64) -> Value {
    json!({
        "instanceArn": instance_arn(REGION, ACCOUNT, instance_id),
        "stat": stat,
        "period": period,
        "startTime": start,
        "endTime": end,
    })
}

/// Read, asserting the call was accepted.
async fn projected(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(
        router,
        "GetEC2RecommendationProjectedMetrics",
        body.clone(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {body}, got {status}: {response}"
    );
    response
}

/// The options a response reports.
fn options_of(case: &str, response: &Value) -> Vec<Value> {
    response["recommendedOptionProjectedMetrics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {response}"))
        .clone()
}

/// The one option a response reports, asserting there is exactly one.
fn only_option(case: &str, response: &Value) -> Value {
    let options = options_of(case, response);
    assert_eq!(
        options.len(),
        1,
        "case {case:?}: the analysis records one candidate, got {response}"
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

/// The values of one projected series.
fn values_of(case: &str, reported: &Value) -> Vec<f64> {
    reported["values"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a list of values, got {reported}"))
        .iter()
        .map(|value| value.as_f64().unwrap_or(f64::NAN))
        .collect()
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
                SIZED_DOWN,
                "Maximum",
                period,
                WINDOW_START,
                WINDOW_START + length,
            ),
        )
        .await;
        let option = only_option(case, &response);
        let cpu = series(&option, "Cpu")
            .unwrap_or_else(|| panic!("case {case:?}: a projected Cpu series, got {option}"));
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
        let values = values_of(case, &cpu);
        assert_eq!(
            values.len(),
            timestamps.len(),
            "case {case:?}: one value per timestamp, got {cpu}"
        );
        // The analysis recorded ONE figure per metric and statistic, so every data point of
        // the window carries the same projected value.
        assert!(
            values.iter().all(|value| close(*value, 40.0)),
            "case {case:?}: 20% of twice the vCPUs is 40% of half of them, got {cpu}"
        );
    }
}

/// One row of the projection table: the case, the instance, the statistic asked for, the
/// instance type the option is expected to name, and the series expected — each metric name
/// with the value it carries at every data point.
type ProjectionRow<'a> = (&'a str, &'a str, &'a str, &'a str, Vec<(&'a str, f64)>);

/// The projection rule: a metric measured as a percentage of a resource the instance type
/// sizes scales by the ratio of the two types' capacities for THAT resource and cannot pass
/// the percentage ceiling, and a metric measuring the workload's own traffic is projected
/// unchanged. Each row differs along the direction and the size of the capacity change, and
/// along which of the two resources changes at all.
#[tokio::test]
async fn each_metric_is_projected_onto_the_recommended_instance_type() {
    let (router, _engine) = emulator_with_corpus().await;

    // (case, instance, stat, recommendedInstanceType, the series expected)
    let rows: Vec<ProjectionRow> = vec![
        (
            "half the capacity doubles each percentage",
            SIZED_DOWN,
            "Maximum",
            "c5.2xlarge",
            vec![("Cpu", 40.0), ("Memory", 60.0)],
        ),
        (
            "the same instance under the other statistic",
            SIZED_DOWN,
            "Average",
            "c5.2xlarge",
            vec![("Cpu", 22.0)],
        ),
        (
            "only the memory changes, so only the memory percentage does",
            MEMORY_ONLY,
            "Maximum",
            "t3.large",
            vec![("Cpu", 46.0), ("Memory", 24.0)],
        ),
        (
            "a projection that would pass the ceiling stops at it",
            FAR_SMALLER,
            "Maximum",
            "r5.large",
            // 10% of 48 vCPUs is 240% of 2 of them, and 5% of 384 GiB is 120% of 16.
            vec![("Cpu", 100.0), ("Memory", 100.0)],
        ),
        (
            "the same capacity projects every percentage unchanged",
            SAME_CAPACITY,
            "Maximum",
            "m6i.xlarge",
            vec![("Cpu", 62.5), ("Memory", 71.0)],
        ),
        (
            "a ratio that is neither a doubling nor a halving",
            SIZED_UP,
            "Maximum",
            "c5.12xlarge",
            // 60% of 36 vCPUs is 45% of 48, and 80% of 72 GiB is 60% of 96.
            vec![("Cpu", 45.0), ("Memory", 60.0)],
        ),
    ];

    for (case, instance_id, stat, recommended_type, expected) in rows {
        let response = projected(
            &router,
            case,
            request(
                instance_id,
                stat,
                3_600,
                WINDOW_START,
                WINDOW_START + 2 * 3_600,
            ),
        )
        .await;
        let option = only_option(case, &response);
        assert_eq!(
            option["recommendedInstanceType"],
            json!(recommended_type),
            "case {case:?}: the option names the type the analysis recorded, got {option}"
        );
        assert_eq!(
            option["rank"],
            json!(1),
            "case {case:?}: the one recorded candidate is the first-ranked one, got {option}"
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
            let values = values_of(case, &reported);
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
/// under another statistic is not reported under the requested one.
#[tokio::test]
async fn the_requested_statistic_selects_the_observations_projected() {
    let (router, _engine) = emulator_with_corpus().await;

    for (case, stat, wanted) in [
        ("the maximum observations", "Maximum", vec!["Cpu", "Memory"]),
        ("the average observations", "Average", vec!["Cpu"]),
    ] {
        let response = projected(
            &router,
            case,
            request(SIZED_DOWN, stat, 3_600, WINDOW_START, WINDOW_START + 3_600),
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

    // An instance the analysis observed nothing on under the requested statistic still has
    // its candidate, and that candidate carries no series rather than an empty one.
    let case = "no observation under the requested statistic";
    let response = projected(
        &router,
        case,
        request(
            MEMORY_ONLY,
            "Average",
            3_600,
            WINDOW_START,
            WINDOW_START + 3_600,
        ),
    )
    .await;
    let option = only_option(case, &response);
    assert_eq!(
        option["recommendedInstanceType"],
        json!("t3.large"),
        "case {case:?}: the option is the one the analysis recorded, got {option}"
    );
    assert!(
        option["projectedMetrics"].is_null(),
        "case {case:?}: nothing was observed under Average, so no series is reported, \
         got {option}"
    );
}

/// "The `Cpu` and `Memory` metrics are the only projected utilization metrics returned when
/// you run this action" (`RecommendedOptionProjectedMetric`): the observations this action
/// projects are exactly the ones measured as a percentage of a resource the instance TYPE
/// sizes, so an observation of the workload's own traffic — which the analysis records all the
/// same — gets no series here, under either statistic and on either candidate.
#[tokio::test]
async fn only_the_metrics_the_instance_type_sizes_are_projected() {
    let (router, _engine) = emulator_with_corpus().await;

    // (case, instance, the traffic metric the analysis observed on it)
    for (case, instance_id, traffic) in [
        ("network bytes", SIZED_DOWN, "NETWORK_IN_BYTES_PER_SECOND"),
        (
            "network packets",
            FAR_SMALLER,
            "NETWORK_PACKETS_IN_PER_SECOND",
        ),
    ] {
        for stat in ["Maximum", "Average"] {
            let response = projected(
                &router,
                case,
                request(instance_id, stat, 3_600, WINDOW_START, WINDOW_START + 3_600),
            )
            .await;
            for option in options_of(case, &response) {
                let reported = reported_names(&option);
                assert!(
                    !reported.contains(&traffic.to_string()),
                    "case {case:?} at {stat:?}: {traffic} measures the workload, not a \
                     resource the instance type sizes, so this action reports no series for \
                     it, got {reported:?}"
                );
                assert!(
                    reported
                        .iter()
                        .all(|name| name == "Cpu" || name == "Memory"),
                    "case {case:?} at {stat:?}: only Cpu and Memory are projected, got \
                     {reported:?}"
                );
            }
        }
    }
}

/// One row of the architecture table: the case, the CPU vendor architectures the request asks
/// for (`None` where it states no preference), and the instance types expected in rank order.
type ArchitectureRow<'a> = (&'a str, Option<Vec<&'a str>>, Vec<&'a str>);

/// `cpuVendorArchitectures` selects which candidates are projected — "Compute Optimizer
/// returns projected utilization metrics for Graviton instance type recommendations only"
/// when a request asks for `AWS_ARM64` — and the options reported are ranked from the top of
/// what the preference admits. A value outside the model's vocabulary names no architecture.
#[tokio::test]
async fn the_requested_cpu_architecture_selects_the_candidates_projected() {
    let (router, _engine) = emulator_with_corpus().await;

    // (case, the preference asked for, the instance types expected in rank order)
    let rows: Vec<ArchitectureRow> = vec![
        (
            "no preference restricts nothing",
            None,
            vec!["m5.xlarge", "m6g.xlarge"],
        ),
        (
            "Graviton only",
            Some(vec![CPU_ARCHITECTURE_AWS_ARM64]),
            vec!["m6g.xlarge"],
        ),
        (
            "the architecture the instance runs today",
            Some(vec![CPU_ARCHITECTURE_CURRENT]),
            vec!["m5.xlarge"],
        ),
        (
            "both architectures",
            Some(vec![CPU_ARCHITECTURE_CURRENT, CPU_ARCHITECTURE_AWS_ARM64]),
            vec!["m5.xlarge", "m6g.xlarge"],
        ),
    ];

    for (case, preference, wanted) in rows {
        let mut body = request(
            TWO_CANDIDATES,
            "Maximum",
            3_600,
            WINDOW_START,
            WINDOW_START + 3_600,
        );
        if let Some(architectures) = &preference {
            body["recommendationPreferences"] =
                json!({"cpuVendorArchitectures": architectures});
        }
        let response = projected(&router, case, body).await;
        let options = options_of(case, &response);
        let reported: Vec<String> = options
            .iter()
            .map(|option| {
                option["recommendedInstanceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported,
            wanted
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the candidates {wanted:?}, got {response}"
        );
        let ranks: Vec<i64> = options
            .iter()
            .map(|option| option["rank"].as_i64().unwrap_or_default())
            .collect();
        let expected_ranks: Vec<i64> = (1..=wanted.len() as i64).collect();
        assert_eq!(
            ranks, expected_ranks,
            "case {case:?}: the top option is ranked 1, got {response}"
        );
        // Both candidates have half the capacity of the instance, so each doubles the
        // observed percentages — the projection follows the candidate, not its architecture.
        for option in &options {
            let cpu = series(option, "Cpu")
                .unwrap_or_else(|| panic!("case {case:?}: a projected Cpu series, got {option}"));
            assert!(
                values_of(case, &cpu).iter().all(|value| close(*value, 48.0)),
                "case {case:?}: 24% of twice the vCPUs is 48% of half of them, got {option}"
            );
        }
    }

    let mut body = request(
        TWO_CANDIDATES,
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );
    body["recommendationPreferences"] = json!({"cpuVendorArchitectures": ["Bicycle"]});
    let (status, response) =
        call(&router, "GetEC2RecommendationProjectedMetrics", body.clone()).await;
    assert_eq!(
        status, 400,
        "a CPU vendor architecture the model does not declare must be refused, got \
         {status}: {response}"
    );
}

/// An instance the analysis would not move has no recommendation option, so there is nothing
/// to project metrics onto.
#[tokio::test]
async fn an_instance_the_analysis_would_not_move_reports_no_option() {
    let (router, _engine) = emulator_with_corpus().await;

    let response = projected(
        &router,
        "no candidate",
        request(
            NOT_MOVED,
            "Maximum",
            3_600,
            WINDOW_START,
            WINDOW_START + 3_600,
        ),
    )
    .await;
    assert_eq!(
        response["recommendedOptionProjectedMetrics"],
        json!([]),
        "no candidate, so nothing to project, got {response}"
    );
}

/// A request that is not a series — a granularity that is not a positive span, a window that
/// does not run forwards — or one that would carry more data points than a response can, is
/// refused with the parameter named; an instance this endpoint holds no analysis for is
/// reported missing, and an identifier that is not an instance ARN, or names another
/// account's instance, is refused.
#[tokio::test]
async fn a_request_that_is_not_a_series_is_refused() {
    let (router, _engine) = emulator_with_corpus().await;
    let window = (WINDOW_START, WINDOW_START + 3_600);
    let with_arn = |arn: String| {
        json!({
            "instanceArn": arn,
            "stat": "Maximum",
            "period": 3_600,
            "startTime": window.0,
            "endTime": window.1,
        })
    };

    // (case, request, the status expected, the error code expected)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "the modelled default granularity divides the window into nothing",
            request(SIZED_DOWN, "Maximum", 0, window.0, window.1),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a granularity that is not a span of time",
            request(SIZED_DOWN, "Maximum", -60, window.0, window.1),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an empty window",
            request(SIZED_DOWN, "Maximum", 3_600, window.0, window.0),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a window that runs backwards",
            request(SIZED_DOWN, "Maximum", 3_600, window.1, window.0),
            400,
            "InvalidParameterValueException",
        ),
        (
            "one data point past the most a response carries",
            request(
                SIZED_DOWN,
                "Maximum",
                60,
                WINDOW_START,
                WINDOW_START + 60 * (MAX_PROJECTED_METRIC_DATA_POINTS as i64 + 1),
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an identifier that is not an ARN",
            with_arn(SIZED_DOWN.to_string()),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an ARN that names no instance",
            with_arn(format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:orders-primary")),
            400,
            "InvalidParameterValueException",
        ),
        (
            "another account's instance",
            with_arn(instance_arn(REGION, "210987654321", SIZED_DOWN)),
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (case, body, status_wanted, code) in rows {
        let (status, response) = call(
            &router,
            "GetEC2RecommendationProjectedMetrics",
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

    // An instance this service holds no analysis of has no projection to report, which is a
    // SUCCESSFUL empty list rather than a failure: a caller charting the projection of each
    // instance renders an empty series instead of aborting.
    let case = "an instance this service holds no analysis of";
    let response = projected(
        &router,
        case,
        request(
            "i-0b1b2c3d4e5f79999",
            "Maximum",
            3_600,
            window.0,
            window.1,
        ),
    )
    .await;
    assert_eq!(
        response["recommendedOptionProjectedMetrics"],
        json!([]),
        "case {case:?}: expected an empty list of projections, got {response}"
    );

    // The ceiling itself is served: the bound is on the response, not on the request's shape.
    let case = "the ceiling itself";
    let response = projected(
        &router,
        case,
        request(
            SIZED_DOWN,
            "Maximum",
            60,
            WINDOW_START,
            WINDOW_START + 60 * MAX_PROJECTED_METRIC_DATA_POINTS as i64,
        ),
    )
    .await;
    let option = only_option(case, &response);
    assert_eq!(
        series(&option, "Cpu")
            .and_then(|metric| metric["timestamps"].as_array().map(Vec::len))
            .unwrap_or_default(),
        MAX_PROJECTED_METRIC_DATA_POINTS,
        "case {case:?}: the ceiling is served in full, got {option}"
    );
}

/// Compute Optimizer is a REGIONAL service: the endpoint a request addresses is the region
/// whose resources it reports on, so an instance named by the ARN of another region is one
/// this endpoint holds no analysis for — and is answered with the empty list of projections.
#[tokio::test]
async fn an_instance_of_another_region_is_not_this_endpoints() {
    let (router, _engine) = emulator_with_corpus().await;
    let body = request(
        SIZED_DOWN,
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );

    // (case, the region the read is made at, whether that endpoint holds the analysis)
    for (case, region, analysed_here) in [
        ("the region the instance was read in", REGION, true),
        ("another commercial region", "eu-west-1", false),
        ("a region on another continent", "ap-southeast-2", false),
    ] {
        let (status, response) = call_in_region(
            &router,
            region,
            "GetEC2RecommendationProjectedMetrics",
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
    record(&engine, corpus());

    let body = request(
        SIZED_DOWN,
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );
    let (status, response) = call(
        &router,
        "GetEC2RecommendationProjectedMetrics",
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
        !options_of("once the analysis is due", &response).is_empty(),
        "once the analysis is due the projection is served, got {response}"
    );
}

/// The instances this emulator has analysed are part of the store's initial value, so the
/// projection of one is computed from that stored analysis — and a read REPORTS: it must not
/// change what a later caller sees.
#[tokio::test]
async fn the_analysed_instance_projects_every_candidate_it_records() {
    let (router, engine) = emulator_with_engine();
    analysed(&router).await;
    let before = analysed_instances(&engine);

    // The instance the analysis would move to half its capacity, with a Graviton candidate
    // beside it: 22.5% of 8 vCPUs is 45% of 4, and 31% of 32 GiB is 62% of 16. The network
    // traffic the analysis also observed on it is of neither resource the instance type
    // sizes, so this action reports no series for it.
    let body = json!({
        "instanceArn": instance_arn(REGION, ACCOUNT, "i-0a1b2c3d4e5f60001"),
        "stat": "Maximum",
        "period": 3_600,
        "startTime": WINDOW_START,
        "endTime": WINDOW_START + 3 * 3_600,
    });
    let case = "the analysed instance";
    let response = projected(&router, case, body.clone()).await;
    let options = options_of(case, &response);
    assert_eq!(
        options.len(),
        2,
        "case {case:?}: both recorded candidates are projected, got {response}"
    );
    assert_eq!(
        options[0]["recommendedInstanceType"],
        json!("m5.xlarge"),
        "case {case:?}: the top-ranked candidate first, got {response}"
    );
    assert_eq!(
        options[1]["recommendedInstanceType"],
        json!("m6g.xlarge"),
        "case {case:?}: the Graviton candidate second, got {response}"
    );
    for (name, wanted) in [("Cpu", 45.0), ("Memory", 62.0)] {
        let reported = series(&options[0], name)
            .unwrap_or_else(|| panic!("case {case:?}: a projected {name} series, got {response}"));
        let values = values_of(case, &reported);
        assert_eq!(
            values.len(),
            3,
            "case {case:?}: three hourly data points, got {reported}"
        );
        assert!(
            values.iter().all(|value| close(*value, wanted)),
            "case {case:?}: metric {name:?}: expected {wanted} at every data point, got \
             {values:?}"
        );
    }

    assert_eq!(
        reported_names(&options[0]),
        vec!["Cpu".to_string(), "Memory".to_string()],
        "case {case:?}: only the metrics the instance type sizes are projected, got \
         {response}"
    );

    let again = projected(&router, "the same read again", body).await;
    assert_eq!(
        again, response,
        "a second read must report the same projection, got {again}"
    );
    assert_eq!(
        analysed_instances(&engine),
        before,
        "a read must not add or drop an analysed instance"
    );
}
