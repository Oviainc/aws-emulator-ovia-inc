// SPDX-License-Identifier: CC0-1.0

//! `GetECSServiceRecommendationProjectedMetrics`, driven end to end over HTTP.
//!
//! The rule these suites pin: the projected metrics of an Amazon ECS service recommendation
//! are COMPUTED from state the emulator holds — the ECS store's service record and its task
//! definition's task-level size, and the `AWS/ECS` `CPUUtilization`/`MemoryUtilization`
//! observations published for the service over the effective look-back period. Concretely:
//!
//! * the recommended option is the smallest Fargate task size that holds each dimension's
//!   observed PEAK plus the service's documented headroom — the very option
//!   `GetECSServiceRecommendations` reports for the same service;
//! * each metric is projected onto that size by the ratio of the current and recommended
//!   sizes of the dimension it measures, because the same work on half the capacity runs at
//!   twice the utilization;
//! * the series spans the requested window at the requested granularity — one data point per
//!   period that begins inside it — and carries the one projected value at each data point,
//!   as both its upper and its lower bound;
//! * `stat` selects which observed statistic is projected;
//! * a service this endpoint has no analysis for — unknown, not on Fargate, or with no
//!   observation inside the window — has no option to project onto, so it is answered with
//!   the empty list of projections.
//!
//! Every row differs along what that rule generalises over: the observed utilization, the
//! current task size and the form it is registered in, the direction the analysis sizes the
//! task in, the statistic asked for, and the window and granularity asked for.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::MAX_PROJECTED_METRIC_DATA_POINTS;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_ecs, call_in_region, emulator_with_analysed_services_and_engine, error_code, opt_in,
    seed_cloudwatch_series, ACCOUNT, REGION,
};

/// The two `AWS/ECS` series an ECS service recommendation is computed from.
const CPU_METRIC: &str = "CPUUtilization";
/// See [`CPU_METRIC`].
const MEMORY_METRIC: &str = "MemoryUtilization";

/// The first data point every window below asks for, in epoch seconds.
const WINDOW_START: i64 = 1_761_000_000;

/// The cluster every fixture service below runs in.
const CLUSTER: &str = "projections";

/// One analysed service: its persisted task size, what was observed for it, and the Fargate
/// task size that follows.
#[derive(Debug)]
struct Case {
    /// What the case is about, named in every assertion of the row.
    name: &'static str,
    /// The ECS service name, and so the last part of its ARN.
    service: &'static str,
    /// The task definition's task-level `cpu`/`memory`, in the form it is registered with.
    cpu: &'static str,
    memory: &'static str,
    /// What those forms mean in CPU units and MiB.
    current_cpu: f64,
    current_memory: f64,
    /// The two `CPUUtilization` and two `MemoryUtilization` observations, as per cent: the
    /// analysis folds the peak and the mean of each pair out of the look-back window.
    cpu_values: [f64; 2],
    memory_values: [f64; 2],
    /// The Fargate task size the observed peaks are sized to.
    option_cpu: f64,
    option_memory: f64,
}

/// The analysed services every suite below reads, each sized to its own Fargate task size:
/// one the analysis sizes DOWN, one already at the smallest sufficient size, one it sizes UP,
/// one whose memory requirement forces a larger CPU row than its CPU requirement needs, and
/// one registered in the `vCPU` string form.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "both dimensions can be sized down",
            service: "overprovisioned",
            cpu: "1024",
            memory: "2048",
            current_cpu: 1024.0,
            current_memory: 2048.0,
            cpu_values: [20.0, 40.0],
            memory_values: [30.0, 40.0],
            option_cpu: 512.0,
            option_memory: 1024.0,
        },
        Case {
            name: "a task already at the smallest sufficient size",
            service: "optimized",
            cpu: "256",
            memory: "512",
            current_cpu: 256.0,
            current_memory: 512.0,
            cpu_values: [50.0, 70.0],
            memory_values: [60.0, 70.0],
            option_cpu: 256.0,
            option_memory: 512.0,
        },
        Case {
            name: "both dimensions past their target size the task up",
            service: "saturated",
            cpu: "512",
            memory: "1024",
            current_cpu: 512.0,
            current_memory: 1024.0,
            cpu_values: [90.0, 99.0],
            memory_values: [95.0, 99.0],
            option_cpu: 1024.0,
            option_memory: 2048.0,
        },
        Case {
            name: "the memory requirement forces a larger cpu row",
            service: "memorybound",
            cpu: "4096",
            memory: "8GB",
            current_cpu: 4096.0,
            current_memory: 8192.0,
            cpu_values: [2.0, 5.0],
            memory_values: [90.0, 95.0],
            option_cpu: 2048.0,
            option_memory: 10_240.0,
        },
        Case {
            name: "a task size registered as vCPUs",
            service: "vcpuform",
            cpu: "1 vCPU",
            memory: "2048",
            current_cpu: 1024.0,
            current_memory: 2048.0,
            cpu_values: [70.0, 80.0],
            memory_values: [5.0, 10.0],
            option_cpu: 1024.0,
            option_memory: 2048.0,
        },
    ]
}

impl Case {
    /// The peak and the mean of one dimension's observations: what the analysis folds out of
    /// the look-back window, computed here independently of the handler.
    fn folded(values: [f64; 2]) -> (f64, f64) {
        (values[0].max(values[1]), (values[0] + values[1]) / 2.0)
    }

    /// The value each metric is projected to take under `stat`, in reported order.
    fn projected(&self, stat: &str) -> Vec<(&'static str, f64)> {
        let (cpu_maximum, cpu_average) = Self::folded(self.cpu_values);
        let (memory_maximum, memory_average) = Self::folded(self.memory_values);
        let (cpu, memory) = if stat == "Maximum" {
            (cpu_maximum, memory_maximum)
        } else {
            (cpu_average, memory_average)
        };
        vec![
            ("Cpu", cpu * self.current_cpu / self.option_cpu),
            ("Memory", memory * self.current_memory / self.option_memory),
        ]
    }
}

/// An emulator holding every [`cases`] service, opted in, with its utilization published.
async fn emulator_with_cases() -> MultiServiceRouter {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    create_cluster(&router, CLUSTER, "setup").await;
    let published_at = now_secs() - 3_600;
    for case in cases() {
        let family = format!("{}family", case.service);
        register_task_definition(&router, &family, case.cpu, case.memory, case.name).await;
        create_service(
            &router,
            CLUSTER,
            case.service,
            &family,
            "FARGATE",
            1,
            case.name,
        )
        .await;
        publish(
            &engine,
            CLUSTER,
            case.service,
            CPU_METRIC,
            &case.cpu_values,
            published_at,
        );
        publish(
            &engine,
            CLUSTER,
            case.service,
            MEMORY_METRIC,
            &case.memory_values,
            published_at,
        );
    }
    router
}

/// The series reported over `[start, end)` at a `period`-second granularity carries one data
/// point per period that BEGINS inside the window, the first at `startTime`, and one upper
/// and one lower bound value per timestamp. Each row differs along the granularity, the
/// window length, and whether the window divides into whole periods.
#[tokio::test]
async fn the_series_spans_the_requested_window_at_the_requested_granularity() {
    let router = emulator_with_cases().await;
    let case = &cases()[0];

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

    for (row, period, length, points) in rows {
        let response = projected_ok(
            &router,
            row,
            request(case.service, "Maximum", period, WINDOW_START, WINDOW_START + length),
        )
        .await;
        let option = only_option(row, &response);
        let series = series_of(row, &option, "Cpu");
        let timestamps: Vec<f64> = numbers(row, &series, "timestamps");
        assert_eq!(
            timestamps.len(),
            points,
            "case {row:?}: expected {points} data points, got {series}"
        );
        let expected: Vec<f64> = (0..points)
            .map(|index| (WINDOW_START + period * index as i64) as f64)
            .collect();
        assert!(
            timestamps
                .iter()
                .zip(&expected)
                .all(|(actual, wanted)| close(*actual, *wanted)),
            "case {row:?}: expected a data point every {period}s from {WINDOW_START}, got \
             {timestamps:?}"
        );
        assert!(
            timestamps.last() < Some(&((WINDOW_START + length) as f64)),
            "case {row:?}: every data point begins inside the window, got {timestamps:?}"
        );
        // The analysis folded ONE figure per metric and statistic out of the look-back
        // window, so every data point of the requested window carries the same projection.
        let wanted = case.projected("Maximum")[0].1;
        for bound in ["upperBoundValues", "lowerBoundValues"] {
            let values: Vec<f64> = numbers(row, &series, bound);
            assert_eq!(
                values.len(),
                timestamps.len(),
                "case {row:?}: one {bound} value per timestamp, got {series}"
            );
            assert!(
                values.iter().all(|value| close(*value, wanted)),
                "case {row:?}: expected {bound} of {wanted} at every data point, got {values:?}"
            );
        }
    }
}

/// The projection rule: the recommended option is the smallest Fargate task size that holds
/// each dimension's observed peak plus the headroom, and each metric is projected onto it by
/// the ratio of the two sizes of the dimension it measures. `stat` selects which observed
/// statistic is projected.
#[tokio::test]
async fn each_observed_statistic_is_projected_onto_the_recommended_task_size() {
    let router = emulator_with_cases().await;

    for case in cases() {
        for stat in ["Maximum", "Average"] {
            let row = format!("{} under {stat}", case.name);
            let response = projected_ok(
                &router,
                &row,
                request(
                    case.service,
                    stat,
                    3_600,
                    WINDOW_START,
                    WINDOW_START + 2 * 3_600,
                ),
            )
            .await;
            let option = only_option(&row, &response);
            assert_eq!(
                option["recommendedCpuUnits"].as_f64(),
                Some(case.option_cpu),
                "case {row:?}: expected the recommended cpu {}, got {option}",
                case.option_cpu
            );
            assert_eq!(
                option["recommendedMemorySize"].as_f64(),
                Some(case.option_memory),
                "case {row:?}: expected the recommended memory {}, got {option}",
                case.option_memory
            );
            let expected = case.projected(stat);
            assert_eq!(
                reported_names(&option),
                expected
                    .iter()
                    .map(|(name, _)| (*name).to_string())
                    .collect::<Vec<String>>(),
                "case {row:?}: expected exactly the analysed metrics, got {option}"
            );
            for (name, wanted) in expected {
                let series = series_of(&row, &option, name);
                for bound in ["upperBoundValues", "lowerBoundValues"] {
                    let values: Vec<f64> = numbers(&row, &series, bound);
                    assert_eq!(
                        values.len(),
                        2,
                        "case {row:?}: two hourly data points were asked for, got {series}"
                    );
                    assert!(
                        values.iter().all(|value| close(*value, wanted)),
                        "case {row:?}: metric {name:?}: expected {bound} of {wanted} at every \
                         data point, got {values:?}"
                    );
                }
            }
        }
    }
}

/// One analysis, two readers: the option this operation projects onto is the option
/// `GetECSServiceRecommendations` reports for the same service, and the values it projects
/// are that option's own projected utilization metrics.
#[tokio::test]
async fn the_projection_agrees_with_the_recommendation_reader() {
    let router = emulator_with_cases().await;

    for case in cases() {
        for stat in ["Maximum", "Average"] {
            let row = format!("{} under {stat}", case.name);
            let arn = service_arn(CLUSTER, case.service);
            let (status, body) = call(
                &router,
                "GetECSServiceRecommendations",
                json!({ "serviceArns": [&arn] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {row:?}: expected 200 from the recommendation reader, got {status}: {body}"
            );
            let recommendation = &body["ecsServiceRecommendations"][0];
            let recommended = &recommendation["serviceRecommendationOptions"][0];

            let response = projected_ok(
                &router,
                &row,
                request(case.service, stat, 3_600, WINDOW_START, WINDOW_START + 3_600),
            )
            .await;
            let option = only_option(&row, &response);
            assert_eq!(
                option["recommendedCpuUnits"], recommended["cpu"],
                "case {row:?}: the two readers report one option's cpu, got {option} and \
                 {recommended}"
            );
            assert_eq!(
                option["recommendedMemorySize"], recommended["memory"],
                "case {row:?}: the two readers report one option's memory, got {option} and \
                 {recommended}"
            );
            for name in ["Cpu", "Memory"] {
                let entry = recommended["projectedUtilizationMetrics"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {row:?}: a projection list, got {recommended}"))
                    .iter()
                    .find(|metric| {
                        metric["name"] == json!(name) && metric["statistic"] == json!(stat)
                    })
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("case {row:?}: a {name} {stat} projection, got {recommended}")
                    });
                let series = series_of(&row, &option, name);
                for (bound, member) in [
                    ("upperBoundValues", "upperBoundValue"),
                    ("lowerBoundValues", "lowerBoundValue"),
                ] {
                    let wanted = entry[member].as_f64().unwrap_or(f64::NAN);
                    let values: Vec<f64> = numbers(&row, &series, bound);
                    assert!(
                        values.iter().all(|value| close(*value, wanted)),
                        "case {row:?}: metric {name:?}: the series carries the option's own \
                         {member} {wanted}, got {values:?}"
                    );
                }
            }
        }
    }
}

/// A request this service cannot answer a series for is refused with the modelled error for
/// the reason it cannot: a parameter outside what the member documents is a 400. A service
/// this endpoint holds no recommendation for is NOT a refusal — it is answered with the
/// empty list of projections.
#[tokio::test]
async fn a_request_with_no_series_to_report_is_refused_or_answered_empty() {
    let router = emulator_with_cases().await;
    let analysed = cases()[0].service;
    // A service that exists but is not one Compute Optimizer analyses: it does not run on
    // Fargate, so it has no recommendation option to project onto.
    register_task_definition(&router, "ec2family", "1024", "2048", "ec2 setup").await;
    create_service(
        &router,
        CLUSTER,
        "onec2",
        "ec2family",
        "EC2",
        1,
        "ec2 setup",
    )
    .await;
    // A Fargate service with nothing published for it: there is no utilization to project.
    register_task_definition(&router, "quietfamily", "1024", "2048", "quiet setup").await;
    create_service(
        &router,
        CLUSTER,
        "quiet",
        "quietfamily",
        "FARGATE",
        1,
        "quiet setup",
    )
    .await;

    let with_arn = |arn: String| {
        json!({
            "serviceArn": arn,
            "stat": "Maximum",
            "period": 3_600,
            "startTime": WINDOW_START,
            "endTime": WINDOW_START + 3_600,
        })
    };

    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a period of no seconds, the member's own default",
            request(analysed, "Maximum", 0, WINDOW_START, WINDOW_START + 3_600),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a period running backwards",
            request(analysed, "Maximum", -60, WINDOW_START, WINDOW_START + 3_600),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a window that ends where it starts",
            request(analysed, "Maximum", 3_600, WINDOW_START, WINDOW_START),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a window running backwards",
            request(analysed, "Maximum", 3_600, WINDOW_START, WINDOW_START - 60),
            400,
            "InvalidParameterValueException",
        ),
        (
            "one data point past the most a response carries",
            request(
                analysed,
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
            with_arn(analysed.to_string()),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an ARN that names no Amazon ECS service",
            with_arn(format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:orders-primary")),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an ECS ARN that names no cluster",
            with_arn(format!("arn:aws:ecs:{REGION}:{ACCOUNT}:service/{analysed}")),
            400,
            "InvalidParameterValueException",
        ),
        (
            "another account's service",
            with_arn(format!(
                "arn:aws:ecs:{REGION}:210987654321:service/{CLUSTER}/{analysed}"
            )),
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (row, body, status_wanted, code) in rows {
        let (status, response) = call(
            &router,
            "GetECSServiceRecommendationProjectedMetrics",
            body.clone(),
        )
        .await;
        assert_eq!(
            status, status_wanted,
            "case {row:?}: {body} must be answered with {status_wanted}, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {row:?}: expected {code}, got {response}"
        );
    }

    // A service this endpoint holds no recommendation for — nothing recorded for it, one
    // Compute Optimizer does not support, or one with no observation in the window — has no
    // projection to report, which is a SUCCESSFUL empty list rather than a failure: a caller
    // charting each service's projection renders an empty series instead of aborting.
    for (row, service) in [
        ("a service the ECS store holds no record of", "neverexisted"),
        ("a service that does not run on Fargate", "onec2"),
        (
            "a service with no observation in the look-back period",
            "quiet",
        ),
    ] {
        let response = projected_ok(
            &router,
            row,
            request(service, "Maximum", 3_600, WINDOW_START, WINDOW_START + 3_600),
        )
        .await;
        assert_eq!(
            response["recommendedOptionProjectedMetrics"],
            json!([]),
            "case {row:?}: expected an empty list of projections, got {response}"
        );
    }

    // The data point ceiling itself is served: the bound is on the response, not on the
    // request's shape.
    let row = "the ceiling itself";
    let response = projected_ok(
        &router,
        row,
        request(
            analysed,
            "Maximum",
            60,
            WINDOW_START,
            WINDOW_START + 60 * MAX_PROJECTED_METRIC_DATA_POINTS as i64,
        ),
    )
    .await;
    let option = only_option(row, &response);
    assert_eq!(
        numbers(row, &series_of(row, &option, "Cpu"), "timestamps").len(),
        MAX_PROJECTED_METRIC_DATA_POINTS,
        "case {row:?}: the ceiling is served in full, got {option}"
    );
}

/// The service analyses an account's resources only once it has opted in: an account that
/// never opted in reads no projection at all.
#[tokio::test]
async fn the_read_waits_for_the_enrollment() {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    let case = &cases()[0];
    create_cluster(&router, CLUSTER, "setup").await;
    let family = format!("{}family", case.service);
    register_task_definition(&router, &family, case.cpu, case.memory, case.name).await;
    create_service(
        &router,
        CLUSTER,
        case.service,
        &family,
        "FARGATE",
        1,
        case.name,
    )
    .await;
    let published_at = now_secs() - 3_600;
    publish(
        &engine,
        CLUSTER,
        case.service,
        CPU_METRIC,
        &case.cpu_values,
        published_at,
    );
    publish(
        &engine,
        CLUSTER,
        case.service,
        MEMORY_METRIC,
        &case.memory_values,
        published_at,
    );

    let body = request(
        case.service,
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );
    let (status, response) = call(
        &router,
        "GetECSServiceRecommendationProjectedMetrics",
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
    let response = projected_ok(&router, "once opted in", body).await;
    assert!(
        !options_of("once opted in", &response).is_empty(),
        "once the account is opted in the projection is served, got {response}"
    );
}

/// Compute Optimizer is a REGIONAL service: the endpoint a request addresses is the region
/// whose resources it reports on, so a service named by the ARN of another region is one this
/// endpoint holds no recommendation for — answered with the empty list of projections. And a
/// READ reports: it must not change what a later caller sees.
#[tokio::test]
async fn a_service_of_another_region_is_not_this_endpoints() {
    let router = emulator_with_cases().await;
    let case = &cases()[0];
    let body = request(
        case.service,
        "Maximum",
        3_600,
        WINDOW_START,
        WINDOW_START + 3_600,
    );

    let first = projected_ok(&router, "the region the service was created in", body.clone()).await;

    for (row, region) in [
        ("another commercial region", "eu-west-1"),
        ("a region on another continent", "ap-southeast-2"),
    ] {
        // The same request at another endpoint: the ARN names this region's service, which
        // that endpoint holds no analysis for.
        let (status, response) = call_in_region(
            &router,
            region,
            "GetECSServiceRecommendationProjectedMetrics",
            body.clone(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: at {region}, expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["recommendedOptionProjectedMetrics"],
            json!([]),
            "case {row:?}: at {region}, this endpoint holds no recommendation for it, so it \
             reports no projection, got {response}"
        );
        // And an ARN naming another region names no service of THIS endpoint.
        let foreign = json!({
            "serviceArn": format!("arn:aws:ecs:{region}:{ACCOUNT}:service/{CLUSTER}/{}", case.service),
            "stat": "Maximum",
            "period": 3_600,
            "startTime": WINDOW_START,
            "endTime": WINDOW_START + 3_600,
        });
        let (status, response) = call(
            &router,
            "GetECSServiceRecommendationProjectedMetrics",
            foreign.clone(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: {foreign} names no service of {REGION}, expected 200, got {status}: \
             {response}"
        );
        assert_eq!(
            response["recommendedOptionProjectedMetrics"],
            json!([]),
            "case {row:?}: {foreign} names no service of {REGION}, so it reports no \
             projection, got {response}"
        );
    }

    let again = projected_ok(&router, "the same read again", body).await;
    assert_eq!(
        again, first,
        "a second read must report the same projection, got {again}"
    );
}

/// A request for one service's projection over one window.
fn request(service: &str, stat: &str, period: i64, start: i64, end: i64) -> Value {
    json!({
        "serviceArn": service_arn(CLUSTER, service),
        "stat": stat,
        "period": period,
        "startTime": start,
        "endTime": end,
    })
}

/// The ARN the ECS emulator mints for one service.
fn service_arn(cluster: &str, service: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT}:service/{cluster}/{service}")
}

/// The current instant, as whole epoch seconds.
fn now_secs() -> i64 {
    i64::try_from(
        ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch seconds fit an i64")
}

/// Read, asserting the call was accepted.
async fn projected_ok(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(
        router,
        "GetECSServiceRecommendationProjectedMetrics",
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
        .unwrap_or_else(|| panic!("case {case:?}: expected a list of options, got {response}"))
        .clone()
}

/// The one option a response reports, asserting there is exactly one: the analysis records
/// one task size it would move the service to.
fn only_option(case: &str, response: &Value) -> Value {
    let options = options_of(case, response);
    assert_eq!(
        options.len(),
        1,
        "case {case:?}: expected exactly one recommendation option, got {response}"
    );
    options[0].clone()
}

/// The metric names an option reports a series for, in the order reported.
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

/// The projected series of one metric name.
fn series_of(case: &str, option: &Value, name: &str) -> Value {
    option["projectedMetrics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list of series, got {option}"))
        .iter()
        .find(|metric| metric["name"] == json!(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a projected {name} series, got {option}"))
}

/// One numeric list member of a series.
fn numbers(case: &str, series: &Value, member: &str) -> Vec<f64> {
    series[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list {member}, got {series}"))
        .iter()
        .map(|value| value.as_f64().unwrap_or(f64::NAN))
        .collect()
}

/// Two computed metric values agree when they agree to a far finer margin than any
/// projection this analysis reports.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// One ECS call that must succeed, returning its body.
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call_ecs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status}: {parsed}"
    );
    parsed
}

/// Create one ECS cluster.
async fn create_cluster(router: &MultiServiceRouter, cluster: &str, case: &str) {
    ecs_ok(
        router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        case,
    )
    .await;
}

/// Register one Fargate-compatible task definition with the given task-level size.
async fn register_task_definition(
    router: &MultiServiceRouter,
    family: &str,
    cpu: &str,
    memory: &str,
    case: &str,
) {
    ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": cpu,
            "memory": memory,
            "containerDefinitions": [{
                "name": "web",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "cpu": 128,
                "memory": 256,
                "memoryReservation": 192,
            }],
        }),
        case,
    )
    .await;
}

/// Create one ECS service running `family` under `launch_type`, returning its ARN.
async fn create_service(
    router: &MultiServiceRouter,
    cluster: &str,
    service: &str,
    family: &str,
    launch_type: &str,
    desired_count: i64,
    case: &str,
) -> String {
    let body = ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": service,
            "taskDefinition": family,
            "desiredCount": desired_count,
            "launchType": launch_type,
            // The revisions above are in the `awsvpc` network mode, whose tasks each get
            // their own elastic network interface, so the launch says which subnets and
            // security groups that interface takes.
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": ["subnet-0123456789abcdef0"],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
            "tags": [{ "key": "env", "value": "test" }],
        }),
        case,
    )
    .await;
    body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a serviceArn, got {body}"))
        .to_string()
}

/// Record `values` of one `AWS/ECS` series of one service, at `at`.
///
/// `AWS/ECS` is a namespace AWS reserves for the service that owns the metric, and no client
/// may publish into it (`PutMetricData` refuses a `Namespace` beginning with `AWS/`), so the
/// utilization these rows project is seeded into the metric store the way ECS writes it.
fn publish(
    engine: &Arc<engine::InMemoryEngine>,
    cluster: &str,
    service: &str,
    metric_name: &str,
    values: &[f64],
    at: i64,
) {
    #[allow(clippy::cast_precision_loss)]
    seed_cloudwatch_series(
        engine,
        "AWS/ECS",
        metric_name,
        &[("ClusterName", cluster), ("ServiceName", service)],
        Some("Percent"),
        at as f64,
        values,
    );
}
