// SPDX-License-Identifier: CC0-1.0

//! `GetECSServiceRecommendations`, driven end to end over HTTP.
//!
//! The rule these suites pin: an Amazon ECS service recommendation is COMPUTED, for every
//! service the request puts in scope, from state the emulator holds —
//!
//! * the current service configuration is the task size and container definitions the ECS
//!   store holds for the service, and its launch type is the service's own;
//! * the utilization metrics are CloudWatch's own statistics of the service's `AWS/ECS`
//!   `CPUUtilization`/`MemoryUtilization` series over the effective look-back period;
//! * the finding, its reason codes, the recommendation option and the performance risk
//!   follow from sizing each dimension so that its observed PEAK leaves the service's
//!   documented default headroom, the option being the smallest Fargate task size that
//!   holds both dimensions;
//! * the option's savings opportunity is the difference between what the current task size
//!   and the recommended one cost per month at the public Fargate per-vCPU-hour and
//!   per-GB-hour prices, for the tasks the service keeps running, and the recommendation
//!   names the pricing source that estimate came from;
//! * the option's container recommendations are the task definition's own container
//!   reservations carried to the recommended task size;
//! * a service the analysis has no input for is never given an invented recommendation: one
//!   Compute Optimizer does not support (it does not run on Fargate, or its task definition
//!   sets no task size) is reported in `errors`, and one the analysis has merely not reached
//!   — unknown, or with no observation inside the window — is absent from the answer.
//!
//! Every row differs along what that rule generalises over: the observed utilization, the
//! current task size and the form it is registered in, how many tasks the service keeps
//! running, the look-back period in force, and which analysis input is missing.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_ecs, call_in_region, emulator_with_analysed_services,
    emulator_with_analysed_services_and_engine, error_code, opt_in, seed_cloudwatch_series, REGION,
};

/// The account id every ARN the emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The two `AWS/ECS` series an ECS service recommendation is computed from.
const CPU_METRIC: &str = "CPUUtilization";
/// See [`CPU_METRIC`].
const MEMORY_METRIC: &str = "MemoryUtilization";

/// The public on-demand Fargate prices every money figure an ECS service recommendation
/// reports is computed from: one vCPU-hour and one GB-hour of Fargate compute, which is how
/// AWS Fargate's pricing is published (per vCPU per hour and per GB per hour).
///
/// Stated here so the expectation is computed independently of the handler; they are the
/// handler's own `FARGATE_PRICE_PER_VCPU_HOUR` / `FARGATE_PRICE_PER_GB_HOUR` and must stay
/// in step with them.
const PRICE_PER_VCPU_HOUR: f64 = 0.040_48;
/// See [`PRICE_PER_VCPU_HOUR`].
const PRICE_PER_GB_HOUR: f64 = 0.004_445;
/// Hours in a month, the factor an hourly price becomes a monthly figure by: 365 × 24 / 12.
const HOURS_PER_MONTH: f64 = 730.0;

/// One row of the analysis table: a service's persisted size, what was observed for it, and
/// the recommendation that follows.
#[derive(Debug)]
struct AnalysisCase {
    name: &'static str,
    service: &'static str,
    /// The task definition's task-level `cpu`/`memory`, in the form it is registered with.
    cpu: &'static str,
    memory: &'static str,
    /// How many tasks the service keeps running — the multiplier between what one task of
    /// the size above costs and what the SERVICE costs.
    desired_count: i64,
    /// What those forms mean in CPU units and MiB — the current configuration the
    /// recommendation must report.
    current_cpu: f64,
    current_memory: f64,
    /// The `CPUUtilization` and `MemoryUtilization` observations, as per cent.
    cpu_values: [f64; 2],
    memory_values: [f64; 2],
    /// The finding, its reason codes IN REPORTED ORDER, and the performance risk.
    finding: &'static str,
    reason_codes: &'static [&'static str],
    risk: &'static str,
    /// The recommended Fargate task size.
    option_cpu: f64,
    option_memory: f64,
}

#[tokio::test]
async fn a_recommendation_is_computed_from_the_service_and_its_observed_utilization() {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    let cluster = "analysis";
    create_cluster(&router, cluster, "setup").await;
    let published_at = now_secs() - 3_600;

    // The rule generalises over the observed utilization and the current task size, so the
    // rows differ along exactly those — including a size registered in the `vCPU`/`GB`
    // string forms, and one whose recommendation has to leave the 4096-CPU-unit row of the
    // Fargate task size catalog for the 8192 one.
    let cases = [
        AnalysisCase {
            name: "both dimensions can be sized down",
            service: "overprovisioned",
            cpu: "1024",
            memory: "2048",
            desired_count: 1,
            current_cpu: 1024.0,
            current_memory: 2048.0,
            cpu_values: [20.0, 40.0],
            memory_values: [30.0, 40.0],
            finding: "Overprovisioned",
            reason_codes: &["CPUOverprovisioned", "MemoryOverprovisioned"],
            risk: "VeryLow",
            option_cpu: 512.0,
            option_memory: 1024.0,
        },
        AnalysisCase {
            name: "a cpu peak above the target sizes the task up",
            service: "cpubound",
            cpu: "1024",
            memory: "2048",
            desired_count: 4,
            current_cpu: 1024.0,
            current_memory: 2048.0,
            cpu_values: [60.0, 95.0],
            memory_values: [40.0, 50.0],
            finding: "Underprovisioned",
            reason_codes: &["CPUUnderprovisioned"],
            risk: "Medium",
            option_cpu: 2048.0,
            option_memory: 4096.0,
        },
        AnalysisCase {
            name: "a task already at the smallest sufficient size is optimized",
            service: "optimized",
            cpu: "256",
            memory: "512",
            desired_count: 2,
            current_cpu: 256.0,
            current_memory: 512.0,
            cpu_values: [50.0, 70.0],
            memory_values: [60.0, 70.0],
            finding: "Optimized",
            reason_codes: &[],
            risk: "VeryLow",
            option_cpu: 256.0,
            option_memory: 512.0,
        },
        AnalysisCase {
            name: "both dimensions above the target",
            service: "saturated",
            cpu: "512",
            memory: "1024",
            desired_count: 2,
            current_cpu: 512.0,
            current_memory: 1024.0,
            cpu_values: [90.0, 99.0],
            memory_values: [95.0, 99.0],
            finding: "Underprovisioned",
            reason_codes: &["CPUUnderprovisioned", "MemoryUnderprovisioned"],
            risk: "High",
            option_cpu: 1024.0,
            option_memory: 2048.0,
        },
        AnalysisCase {
            name: "only memory is excessive, size given as vCPU and GB",
            service: "memoryheavy",
            cpu: "1 vCPU",
            memory: "8GB",
            desired_count: 3,
            current_cpu: 1024.0,
            current_memory: 8192.0,
            cpu_values: [70.0, 80.0],
            memory_values: [5.0, 10.0],
            finding: "Overprovisioned",
            reason_codes: &["MemoryOverprovisioned"],
            risk: "VeryLow",
            option_cpu: 1024.0,
            option_memory: 2048.0,
        },
        AnalysisCase {
            name: "a memory peak past the 4 vCPU row's largest task memory",
            service: "largest",
            cpu: "4096",
            memory: "30GB",
            desired_count: 1,
            current_cpu: 4096.0,
            current_memory: 30_720.0,
            cpu_values: [40.0, 50.0],
            memory_values: [90.0, 95.0],
            finding: "Underprovisioned",
            reason_codes: &["MemoryUnderprovisioned"],
            risk: "Medium",
            option_cpu: 8192.0,
            option_memory: 36_864.0,
        },
        AnalysisCase {
            // One dimension can be sized DOWN while the other must be sized UP: the
            // reason codes describe each dimension on its own, so the CPU code is
            // reported even though the memory requirement forces the recommended task to
            // a LARGER CPU row than the CPU requirement alone needs.
            name: "cpu can shrink while memory must grow",
            service: "mixed",
            cpu: "4096",
            memory: "8GB",
            desired_count: 7,
            current_cpu: 4096.0,
            current_memory: 8192.0,
            cpu_values: [2.0, 5.0],
            memory_values: [90.0, 95.0],
            finding: "Underprovisioned",
            reason_codes: &["CPUOverprovisioned", "MemoryUnderprovisioned"],
            risk: "Medium",
            option_cpu: 2048.0,
            option_memory: 10_240.0,
        },
        AnalysisCase {
            // A service that keeps NO task running costs nothing, so there is nothing to
            // save however oversized its task size is: the saving is zero, and so is the
            // percentage of a monthly cost of zero.
            name: "an oversized service that keeps no task running",
            service: "notasks",
            cpu: "2048",
            memory: "4096",
            desired_count: 0,
            current_cpu: 2048.0,
            current_memory: 4096.0,
            cpu_values: [10.0, 20.0],
            memory_values: [10.0, 20.0],
            finding: "Overprovisioned",
            reason_codes: &["CPUOverprovisioned", "MemoryOverprovisioned"],
            risk: "VeryLow",
            option_cpu: 512.0,
            option_memory: 1024.0,
        },
    ];

    for test in &cases {
        let case = test.name;
        let definition = register_task_definition(
            &router,
            &format!("{}family", test.service),
            test.cpu,
            test.memory,
            case,
        )
        .await;
        let arn = create_service(
            &router,
            cluster,
            test.service,
            &format!("{}family", test.service),
            "FARGATE",
            test.desired_count,
            case,
        )
        .await;
        publish(
            &engine,
            cluster,
            test.service,
            CPU_METRIC,
            &test.cpu_values,
            published_at,
        );
        publish(
            &engine,
            cluster,
            test.service,
            MEMORY_METRIC,
            &test.memory_values,
            published_at,
        );

        let found = only_recommendation(&router, &arn, case).await;

        assert_eq!(
            found["serviceArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the recommendation of {arn}, got {found}"
        );
        assert_eq!(
            found["accountId"].as_str(),
            Some(ACCOUNT),
            "case {case:?}: expected the owning account {ACCOUNT}, got {found}"
        );
        assert_eq!(
            found["launchType"].as_str(),
            Some("Fargate"),
            "case {case:?}: expected the service's own launch type, got {found}"
        );
        // The current configuration is the task definition's, not a restatement of the
        // request: the task size, the task definition ARN and the container definitions.
        let configuration = &found["currentServiceConfiguration"];
        assert_eq!(
            configuration["cpu"].as_f64(),
            Some(test.current_cpu),
            "case {case:?}: expected the persisted cpu {}, got {configuration}",
            test.current_cpu
        );
        assert_eq!(
            configuration["memory"].as_f64(),
            Some(test.current_memory),
            "case {case:?}: expected the persisted memory {}, got {configuration}",
            test.current_memory
        );
        assert_eq!(
            configuration["taskDefinitionArn"].as_str(),
            Some(definition.as_str()),
            "case {case:?}: expected the task definition {definition}, got {configuration}"
        );
        assert_eq!(
            configuration["containerConfigurations"],
            json!([{
                "containerName": "web",
                "cpu": 128,
                "memorySizeConfiguration": { "memory": 256, "memoryReservation": 192 },
            }]),
            "case {case:?}: expected the stored container definitions, got {configuration}"
        );
        assert_eq!(
            found["tags"],
            json!([{ "key": "env", "value": "test" }]),
            "case {case:?}: expected the service's own tags, got {found}"
        );

        // The utilization metrics are the observations' statistics, nothing else.
        let metrics = list(&found, "utilizationMetrics", case);
        for (name, values) in [
            ("Cpu", test.cpu_values),
            ("Memory", test.memory_values),
        ] {
            let maximum = values[0].max(values[1]);
            let average = (values[0] + values[1]) / 2.0;
            assert_close(
                metric_value(&metrics, name, "Maximum", case),
                maximum,
                case,
                &format!("the {name} Maximum over the look-back period"),
            );
            assert_close(
                metric_value(&metrics, name, "Average", case),
                average,
                case,
                &format!("the {name} Average over the look-back period"),
            );
        }
        assert_eq!(
            found["lastRefreshTimestamp"].as_f64(),
            Some(published_at as f64),
            "case {case:?}: the analysis is current to its newest observation, got {found}"
        );
        // "The number of days the Amazon ECS service utilization metrics were analyzed":
        // the default look-back period, which no preference overrides here.
        assert_close(
            found["lookbackPeriodInDays"].as_f64().unwrap_or_default(),
            14.0,
            case,
            "the analysed look-back period in days",
        );
        assert_eq!(
            found["effectiveRecommendationPreferences"]["lookBackPeriod"].as_str(),
            Some("DAYS_14"),
            "case {case:?}: expected the effective look-back preference, got {found}"
        );

        assert_eq!(
            found["finding"].as_str(),
            Some(test.finding),
            "case {case:?}: expected the finding {}, got {found}",
            test.finding
        );
        assert_eq!(
            found["findingReasonCodes"],
            json!(test.reason_codes),
            "case {case:?}: expected the reason codes {:?}, got {found}",
            test.reason_codes
        );
        assert_eq!(
            found["currentPerformanceRisk"].as_str(),
            Some(test.risk),
            "case {case:?}: expected the performance risk {}, got {found}",
            test.risk
        );

        // One recommendation option: the smallest Fargate task size that holds both
        // dimensions, with the utilization each observation becomes at that size.
        let options = list(&found, "serviceRecommendationOptions", case);
        assert_eq!(
            options.len(),
            1,
            "case {case:?}: expected exactly one recommendation option, got {found}"
        );
        let option = &options[0];
        assert_eq!(
            option["cpu"].as_f64(),
            Some(test.option_cpu),
            "case {case:?}: expected the recommended cpu {}, got {option}",
            test.option_cpu
        );
        assert_eq!(
            option["memory"].as_f64(),
            Some(test.option_memory),
            "case {case:?}: expected the recommended memory {}, got {option}",
            test.option_memory
        );
        // The money the recommendation is for: what the two task sizes cost per month at
        // the public Fargate prices, for the tasks this service keeps running. A
        // recommendation that costs the same or more saves nothing rather than a negative
        // amount.
        let current_cost = monthly_cost(test.current_cpu, test.current_memory, test.desired_count);
        let option_cost = monthly_cost(test.option_cpu, test.option_memory, test.desired_count);
        let expected_saving = round2((current_cost - option_cost).max(0.0));
        let expected_percentage = if current_cost == 0.0 {
            0.0
        } else {
            round2(expected_saving / current_cost * 100.0)
        };
        let savings = &option["savingsOpportunity"];
        assert!(
            (savings["estimatedMonthlySavings"]["value"]
                .as_f64()
                .unwrap_or(f64::NAN)
                - expected_saving)
                .abs()
                < 0.01,
            "case {case:?}: expected a monthly saving of {expected_saving}, got {savings}"
        );
        assert_eq!(
            savings["estimatedMonthlySavings"]["currency"].as_str(),
            Some("USD"),
            "case {case:?}: expected the saving in USD, got {savings}"
        );
        assert!(
            (savings["savingsOpportunityPercentage"]
                .as_f64()
                .unwrap_or(f64::NAN)
                - expected_percentage)
                .abs()
                < 0.01,
            "case {case:?}: expected a saving of {expected_percentage}% of the current monthly \
             cost, got {savings}"
        );
        // The pricing source those figures came from.
        assert_eq!(
            found["effectiveRecommendationPreferences"]["savingsEstimationMode"]["source"].as_str(),
            Some("PublicPricing"),
            "case {case:?}: expected the pricing source of the estimate, got {found}"
        );
        // No savings-estimation-mode preference is in force here, so no after-discount
        // saving is reported (the preference is what activates it).
        assert!(
            option["savingsOpportunityAfterDiscounts"].is_null(),
            "case {case:?}: no savings-estimation-mode preference is set, so no after-discount \
             saving is reported, got {option}"
        );

        // The container recommendations are the task definition's own reservations carried
        // to the recommended task size, in the share of the task they reserve today.
        assert_eq!(
            option["containerRecommendations"],
            json!([{
                "containerName": "web",
                "cpu": scaled(128.0, test.current_cpu, test.option_cpu),
                "memorySizeConfiguration": {
                    "memory": scaled(256.0, test.current_memory, test.option_memory),
                    "memoryReservation": scaled(192.0, test.current_memory, test.option_memory),
                },
            }]),
            "case {case:?}: expected the container reservations at the recommended task size, \
             got {option}"
        );
        let projected = list(option, "projectedUtilizationMetrics", case);
        for (name, values, current, recommended) in [
            ("Cpu", test.cpu_values, test.current_cpu, test.option_cpu),
            (
                "Memory",
                test.memory_values,
                test.current_memory,
                test.option_memory,
            ),
        ] {
            let maximum = values[0].max(values[1]);
            let entry = metric_entry(&projected, name, "Maximum", case);
            let expected = maximum * current / recommended;
            for bound in ["lowerBoundValue", "upperBoundValue"] {
                assert_close(
                    entry[bound].as_f64().unwrap_or_default(),
                    expected,
                    case,
                    &format!("the projected {name} Maximum {bound}"),
                );
            }
        }
    }

    // A request that names no ARN at all reports every analysed service, in ARN order.
    let (status, body) = call(&router, "GetECSServiceRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "the inventory sweep: expected 200, got {status}: {body}"
    );
    let mut arns: Vec<String> = list(&body, "ecsServiceRecommendations", "sweep")
        .iter()
        .map(|found| found["serviceArn"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected: Vec<String> = cases
        .iter()
        .map(|test| service_arn(cluster, test.service))
        .collect();
    arns.sort();
    expected.sort();
    assert_eq!(
        arns, expected,
        "the inventory sweep: expected every analysed service, got {body}"
    );
}

/// The after-discount savings opportunity is reported only once the account activates the
/// savings-estimation-mode preference for this resource type; the figures are the on-demand
/// ones, because no Savings Plans commitment is recorded against a service's Fargate spend.
#[tokio::test]
async fn after_discount_savings_are_reported_only_when_the_preference_activates_them() {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    let cluster = "discounts";
    create_cluster(&router, cluster, "setup").await;
    let published_at = now_secs() - 3_600;

    // Two oversized services, so every row is observed on more than one recommendation.
    let services = ["alpha", "bravo"];
    for service in services {
        let family = format!("{service}family");
        register_task_definition(&router, &family, "2048", "4096", service).await;
        create_service(&router, cluster, service, &family, "FARGATE", 2, service).await;
        for metric in [CPU_METRIC, MEMORY_METRIC] {
            publish(&engine, cluster, service, metric, &[10.0, 20.0], published_at);
        }
    }

    // Before the preference is set, neither service carries an after-discount saving.
    for service in services {
        let arn = service_arn(cluster, service);
        let found = only_recommendation(&router, &arn, "the preference unset").await;
        let option = list(&found, "serviceRecommendationOptions", service)[0].clone();
        assert!(
            option["savingsOpportunityAfterDiscounts"].is_null(),
            "case {service:?}: the preference is unset, so no after-discount saving is \
             reported, got {option}"
        );
    }

    // The rows are the states the preference can be put in, applied in order: one set for
    // ANOTHER resource type (which this resource type's recommendation does not read), the
    // deactivating value, and the activating one.
    let rows: [(&str, Value, bool); 3] = [
        (
            "the preference of another resource type",
            json!({
                "resourceType": "LambdaFunction",
                "savingsEstimationMode": "AfterDiscounts",
            }),
            false,
        ),
        (
            "BeforeDiscounts deactivates the preference",
            json!({
                "resourceType": "EcsService",
                "savingsEstimationMode": "BeforeDiscounts",
            }),
            false,
        ),
        (
            "AfterDiscounts activates the preference",
            json!({
                "resourceType": "EcsService",
                "savingsEstimationMode": "AfterDiscounts",
            }),
            true,
        ),
    ];
    for (case, request, activated) in rows {
        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {request} to answer 200, got {status}: {body}"
        );

        for service in services {
            let arn = service_arn(cluster, service);
            let found = only_recommendation(&router, &arn, case).await;
            let options = list(&found, "serviceRecommendationOptions", case);
            let option = &options[0];
            let on_demand = &option["savingsOpportunity"];
            let after = &option["savingsOpportunityAfterDiscounts"];
            assert!(
                on_demand["estimatedMonthlySavings"]["value"]
                    .as_f64()
                    .is_some_and(|value| value > 0.0),
                "case {case:?} service {service:?}: an oversized service saves money, got {option}"
            );
            if activated {
                // No discount is recorded against Fargate spend, so the after-discount
                // saving is the on-demand one, in the same currency.
                assert_eq!(
                    after, on_demand,
                    "case {case:?} service {service:?}: with no recorded discount the \
                     after-discount saving is the on-demand one, got {option}"
                );
            } else {
                assert!(
                    after.is_null(),
                    "case {case:?} service {service:?}: the preference does not activate an \
                     after-discount saving here, got {option}"
                );
            }
        }
    }
}

/// One row of the no-analysis table: which analysis input is missing, and how the request
/// is answered for the service that misses it.
#[derive(Debug)]
struct ErrorCase {
    name: &'static str,
    service: &'static str,
    /// The ECS launch type the service is created with, or `None` to create no service.
    launch_type: Option<&'static str>,
    /// Which series to publish, and how long ago.
    publish: &'static [&'static str],
    age_secs: i64,
    /// The `GetRecommendationError$code` the request is answered with, or `None` when the
    /// service is simply ABSENT from the answer: `errors` carries the resources the
    /// analysis REJECTED as unsupported, not the ones it has not analysed yet.
    code: Option<&'static str>,
}

#[tokio::test]
async fn a_service_with_no_analysis_input_is_absent_or_reported_as_unsupported() {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    let cluster = "errors";
    create_cluster(&router, cluster, "setup").await;

    // A recommendation needs an ACTIVE Fargate service and both of its series inside the
    // look-back window; the rows differ along which of those is missing.
    let cases = [
        ErrorCase {
            name: "no such service",
            service: "missing",
            launch_type: None,
            publish: &[],
            age_secs: 3_600,
            code: None,
        },
        ErrorCase {
            name: "the service does not run on Fargate",
            service: "oncontainerinstances",
            launch_type: Some("EC2"),
            publish: &[CPU_METRIC, MEMORY_METRIC],
            age_secs: 3_600,
            code: Some("UnsupportedResource"),
        },
        ErrorCase {
            name: "nothing was ever published for the service",
            service: "unobserved",
            launch_type: Some("FARGATE"),
            publish: &[],
            age_secs: 3_600,
            code: None,
        },
        ErrorCase {
            name: "only one of the two series was published",
            service: "cpuonly",
            launch_type: Some("FARGATE"),
            publish: &[CPU_METRIC],
            age_secs: 3_600,
            code: None,
        },
        ErrorCase {
            name: "every observation is older than the look-back period",
            service: "stale",
            launch_type: Some("FARGATE"),
            publish: &[CPU_METRIC, MEMORY_METRIC],
            // 20 days back, which the default DAYS_14 window does not reach.
            age_secs: 20 * 86_400,
            code: None,
        },
    ];

    for test in &cases {
        let case = test.name;
        let family = format!("{}family", test.service);
        if let Some(launch_type) = test.launch_type {
            register_task_definition(&router, &family, "512", "1024", case).await;
            create_service(&router, cluster, test.service, &family, launch_type, 1, case).await;
        }
        for metric in test.publish {
            publish(
                &engine,
                cluster,
                test.service,
                metric,
                &[50.0, 60.0],
                now_secs() - test.age_secs,
            );
        }

        let arn = service_arn(cluster, test.service);
        let (status, body) = call(
            &router,
            "GetECSServiceRecommendations",
            json!({ "serviceArns": [arn] }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert!(
            list(&body, "ecsServiceRecommendations", case).is_empty(),
            "case {case:?}: expected no recommendation to be invented, got {body}"
        );
        let errors = list(&body, "errors", case);
        match test.code {
            Some(code) => {
                assert_eq!(
                    errors.len(),
                    1,
                    "case {case:?}: expected exactly one error entry, got {body}"
                );
                assert_eq!(
                    errors[0]["identifier"].as_str(),
                    Some(arn.as_str()),
                    "case {case:?}: expected the error to name {arn}, got {body}"
                );
                assert_eq!(
                    errors[0]["code"].as_str(),
                    Some(code),
                    "case {case:?}: expected the code {code}, got {body}"
                );
                assert!(
                    errors[0]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(&arn)),
                    "case {case:?}: expected a message naming {arn}, got {body}"
                );
            }
            None => assert!(
                errors.is_empty(),
                "case {case:?}: a service with no recommendation yet is absent from the \
                 answer rather than an error of the request, got {body}"
            ),
        }
    }

    // `maxResults` bounds the whole response: a page of services Compute Optimizer does not
    // support carries at most that many error entries, plus a token for the rest.
    let unsupported = ["notfargateone", "notfargatetwo", "notfargatethree"];
    for service in unsupported {
        let family = format!("{service}family");
        register_task_definition(&router, &family, "512", "1024", service).await;
        create_service(&router, cluster, service, &family, "EC2", 1, service).await;
    }
    let requested: Vec<String> = unsupported
        .iter()
        .map(|service| service_arn(cluster, service))
        .collect();
    let (status, body) = call(
        &router,
        "GetECSServiceRecommendations",
        json!({ "serviceArns": requested, "maxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a bounded page of error entries: expected 200, got {status}: {body}"
    );
    assert_eq!(
        list(&body, "errors", "a bounded page of error entries").len(),
        1,
        "a bounded page of error entries: maxResults bounds the error channel too, got {body}"
    );
    assert!(
        body["nextToken"].is_string(),
        "a bounded page of error entries: expected a token for the remaining services, got {body}"
    );

    // The window is the EFFECTIVE look-back period, so widening the preference to DAYS_32
    // brings the 20-day-old observations of the `stale` service into the analysis — the
    // same request, answered from the same observations, under a wider window.
    let (status, body) = call(
        &router,
        "PutRecommendationPreferences",
        json!({ "resourceType": "EcsService", "lookBackPeriod": "DAYS_32" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "widening the look-back preference: expected 200, got {status}: {body}"
    );
    let arn = service_arn(cluster, "stale");
    let found = only_recommendation(&router, &arn, "a wider look-back period").await;
    assert_close(
        found["lookbackPeriodInDays"].as_f64().unwrap_or_default(),
        32.0,
        "a wider look-back period",
        "the analysed look-back period in days",
    );
    assert_eq!(
        found["effectiveRecommendationPreferences"]["lookBackPeriod"].as_str(),
        Some("DAYS_32"),
        "a wider look-back period: expected the stored preference to be reported, got {found}"
    );
}

#[tokio::test]
async fn a_malformed_request_is_refused_and_an_unenrolled_account_has_no_analysis() {
    let router = emulator_with_analysed_services();
    let arn = service_arn("any", "service");

    // Until the account is opted in there is no analysis at all, whatever the request asks
    // for — the gate every read of this service shares.
    let (status, body) = call(&router, "GetECSServiceRecommendations", json!({})).await;
    assert_eq!(
        status, 403,
        "an account that is not opted in: expected 403, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("OptInRequiredException"),
        "an account that is not opted in: expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;

    // Every row is a request the service defines no answer for; each must be refused with
    // the modelled 400, never with an empty page — and with the ONE bad-input error class
    // this service models, `InvalidParameterValueException`, whether the member is one the
    // MODEL constrains (a filter name outside `ECSServiceRecommendationFilterName`, refused
    // by the constraint walk before the handler runs) or one the model leaves open (refused
    // by the handler itself).
    let cases: [(&str, Value, &str); 7] = [
        (
            "two account ids",
            json!({ "accountIds": [ACCOUNT, "111122223333"] }),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({ "accountIds": ["12345"] }),
            "InvalidParameterValueException",
        ),
        (
            "a service ARN that is not an ECS service ARN",
            json!({ "serviceArns": [format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef0")] }),
            "InvalidParameterValueException",
        ),
        (
            "a service ARN without its cluster",
            json!({ "serviceArns": [format!("arn:aws:ecs:{REGION}:{ACCOUNT}:service/onlyaname")] }),
            "InvalidParameterValueException",
        ),
        (
            "a filter name the service does not define",
            json!({ "filters": [{ "name": "LaunchType", "values": ["Fargate"] }] }),
            "InvalidParameterValueException",
        ),
        (
            "a filter value outside the finding enum",
            json!({ "filters": [{ "name": "Finding", "values": ["NotOptimized"] }] }),
            "InvalidParameterValueException",
        ),
        (
            "a nextToken this service never issued",
            json!({ "nextToken": "not-a-token" }),
            "InvalidParameterValueException",
        ),
    ];
    for (case, request, expected) in cases {
        let (status, body) = call(&router, "GetECSServiceRecommendations", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused with 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains(expected),
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // A well-formed request for a service nothing holds is NOT a refusal, and not an error
    // entry either: the service has no recommendation yet, so it is absent from a successful
    // empty answer.
    let (status, body) = call(
        &router,
        "GetECSServiceRecommendations",
        json!({ "serviceArns": [arn] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a well-formed ARN for an unknown service: expected 200, got {status}: {body}"
    );
    assert!(
        list(&body, "ecsServiceRecommendations", "unknown service").is_empty(),
        "a well-formed ARN for an unknown service: expected no recommendation, got {body}"
    );
    assert!(
        list(&body, "errors", "unknown service").is_empty(),
        "a well-formed ARN for an unknown service: expected no error entry, got {body}"
    );
}

#[tokio::test]
async fn recommendations_are_paged_in_arn_order_and_filtered_by_finding() {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    let cluster = "paging";
    create_cluster(&router, cluster, "setup").await;
    let published_at = now_secs() - 3_600;

    // Three services: two are oversized for what they run, one is short of CPU.
    let services: [(&str, [f64; 2]); 3] = [
        ("alpha", [20.0, 30.0]),
        ("bravo", [20.0, 30.0]),
        ("charlie", [90.0, 99.0]),
    ];
    for (service, cpu_values) in services {
        let family = format!("{service}family");
        register_task_definition(&router, &family, "1024", "2048", service).await;
        create_service(&router, cluster, service, &family, "FARGATE", 1, service).await;
        publish(
            &engine,
            cluster,
            service,
            CPU_METRIC,
            &cpu_values,
            published_at,
        );
        publish(
            &engine,
            cluster,
            service,
            MEMORY_METRIC,
            &[20.0, 30.0],
            published_at,
        );
    }

    // Every page size walks the same sequence, in ARN order, and the last page carries no
    // token.
    for page_size in [1_usize, 2, 3] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "GetECSServiceRecommendations", request).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
            let page = list(&body, "ecsServiceRecommendations", &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page may hold at most {page_size} recommendations, got {body}"
            );
            seen.extend(
                page.iter()
                    .map(|found| found["serviceArn"].as_str().unwrap_or_default().to_string()),
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let expected: Vec<String> = services
            .iter()
            .map(|(service, _)| service_arn(cluster, service))
            .collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every recommendation once, in ARN order"
        );
    }

    // A filter selects by the computed finding and by a computed reason code — not by
    // anything the request supplied.
    for (case, filters, expected) in [
        (
            "Finding=Overprovisioned",
            json!([{ "name": "Finding", "values": ["Overprovisioned"] }]),
            vec![
                service_arn(cluster, "alpha"),
                service_arn(cluster, "bravo"),
            ],
        ),
        (
            "FindingReasonCode=CPUUnderprovisioned",
            json!([{ "name": "FindingReasonCode", "values": ["CPUUnderprovisioned"] }]),
            vec![service_arn(cluster, "charlie")],
        ),
        (
            "Finding=Optimized matches nothing here",
            json!([{ "name": "Finding", "values": ["Optimized"] }]),
            Vec::new(),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetECSServiceRecommendations",
            json!({ "filters": filters }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        let selected: Vec<String> = list(&body, "ecsServiceRecommendations", case)
            .iter()
            .map(|found| found["serviceArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
    }
}

/// Compute Optimizer is a REGIONAL service: the region a request names (its SigV4
/// credential scope) decides which Amazon ECS services are in scope, so a service is
/// analysed only at the endpoint its own ARN names. The rows are other regions the same
/// account can address — the rule holds for each of them, not just one.
#[tokio::test]
async fn only_the_services_of_the_requested_region_are_analysed() {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    let cluster = "regional";
    create_cluster(&router, cluster, "setup").await;
    let family = "regionalfamily";
    register_task_definition(&router, family, "512", "1024", "setup").await;
    let arn = create_service(&router, cluster, "checkout", family, "FARGATE", 1, "setup").await;
    let published_at = now_secs() - 3_600;
    for metric in [CPU_METRIC, MEMORY_METRIC] {
        publish(&engine, cluster, "checkout", metric, &[40.0, 50.0], published_at);
    }

    // At the endpoint the service's ARN names, the sweep reports exactly it.
    let case = format!("the sweep at {REGION}");
    let (status, body) = call(&router, "GetECSServiceRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status}: {body}"
    );
    let reported = list(&body, "ecsServiceRecommendations", &case);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the one service of this region, got {body}"
    );
    assert_eq!(
        reported[0]["serviceArn"].as_str(),
        Some(arn.as_str()),
        "case {case:?}: expected {arn}, got {body}"
    );

    for region in ["eu-west-1", "ap-southeast-2", "us-west-2"] {
        // The same sweep at another endpoint reaches none of this region's services.
        let case = format!("the sweep at {region}");
        let (status, body) = call_in_region(
            &router,
            region,
            "GetECSServiceRecommendations",
            json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert!(
            list(&body, "ecsServiceRecommendations", &case).is_empty(),
            "case {case:?}: a service of another region is not analysed here, got {body}"
        );

        // And an ARN naming another region names no service of THIS endpoint, so it is
        // absent from the answer exactly as an ARN naming nothing is.
        let foreign = format!("arn:aws:ecs:{region}:{ACCOUNT}:service/{cluster}/checkout");
        let case = format!("an ARN naming {region}");
        let (status, body) = call(
            &router,
            "GetECSServiceRecommendations",
            json!({ "serviceArns": [&foreign] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert!(
            list(&body, "ecsServiceRecommendations", &case).is_empty(),
            "case {case:?}: expected no recommendation, got {body}"
        );
        assert!(
            list(&body, "errors", &case).is_empty(),
            "case {case:?}: expected no error entry for {foreign}, got {body}"
        );
    }
}

/// Well-formed 12-digit account ids that are NOT the account this emulator answers for: the
/// id the report cites, the documentation's own example account, and the extremes of the
/// 12-digit range ("a 12-digit Amazon Web Services account ID", `Scope$value`).
const OTHER_ACCOUNTS: [&str; 4] = ["210987654321", "111122223333", "000000000001", "999999999999"];

/// An Amazon ECS service recommendation is reported only for the account the request asks
/// about: the one `accountIds` names, or the caller's own when it names none. The account
/// that OWNS a service is the account field of the service's own ARN, so a service record
/// held under another account's ARN is analysed for no caller of this account — and a
/// well-formed id naming another account is REFUSED rather than used to narrow the answer,
/// because this emulator presents one standalone account and records no organization.
///
/// The rows are well-formed ids of other accounts ([`OTHER_ACCOUNTS`]); each moves one
/// service of this region's inventory under that account and asks both ways.
#[tokio::test]
async fn only_the_services_of_the_requested_account_are_analysed() {
    assert!(
        !OTHER_ACCOUNTS.contains(&ACCOUNT),
        "the other-account rows must not name the account this service answers for \
         ({ACCOUNT}), got {OTHER_ACCOUNTS:?}"
    );
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    let cluster = "ownership";
    create_cluster(&router, cluster, "setup").await;
    let family = "ownershipfamily";
    register_task_definition(&router, family, "1024", "2048", "setup").await;
    let published_at = now_secs() - 3_600;
    let mut both = Vec::new();
    for service in ["kept", "handedover"] {
        both.push(create_service(&router, cluster, service, family, "FARGATE", 1, "setup").await);
        for metric in [CPU_METRIC, MEMORY_METRIC] {
            publish(&engine, cluster, service, metric, &[20.0, 30.0], published_at);
        }
    }
    both.sort();
    let kept = vec![service_arn(cluster, "kept")];

    // Both services belong to the account this endpoint answers for, so the sweep analyses
    // both: every row below changes exactly one thing, which account owns one of them.
    assert_eq!(
        swept(&router, "both services of this account").await,
        both,
        "case \"both services of this account\": expected every service of this account to be \
         analysed, got a different set"
    );

    for account_id in OTHER_ACCOUNTS {
        own_service_in_account(&engine, cluster, "handedover", account_id);

        let case = format!("a service owned by {account_id}");
        assert_eq!(
            swept(&router, &case).await,
            kept,
            "case {case:?}: a service another account owns is analysed for none of this \
             account's requests"
        );

        // Asking for that account by name is refused, so the service is not reachable by
        // naming its owner either.
        let (status, body) = call(
            &router,
            "GetECSServiceRecommendations",
            json!({ "accountIds": [account_id] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected naming another account to be refused with 400, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );

        // Naming this account explicitly reads exactly what the sweep read.
        let (status, body) = call(
            &router,
            "GetECSServiceRecommendations",
            json!({ "accountIds": [ACCOUNT] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for this account, got {status}: {body}"
        );
        let reported: Vec<String> = list(&body, "ecsServiceRecommendations", &case)
            .iter()
            .map(|found| found["serviceArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, kept,
            "case {case:?}: naming this account reads this account's services only, got {body}"
        );
    }

    // Handed back, the service is analysed again — so the rows above left it out for the
    // account that owned it, not because moving it damaged the record.
    own_service_in_account(&engine, cluster, "handedover", ACCOUNT);
    assert_eq!(
        swept(&router, "handed back to this account").await,
        both,
        "case \"handed back to this account\": expected every service of this account to be \
         analysed again, got a different set"
    );
}

/// The `serviceArn`s one sweep (a request naming no ARN and no account) reports, in ARN
/// order.
async fn swept(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "GetECSServiceRecommendations", json!({})).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    let mut reported: Vec<String> = list(&body, "ecsServiceRecommendations", case)
        .iter()
        .map(|found| found["serviceArn"].as_str().unwrap_or_default().to_string())
        .collect();
    reported.sort();
    reported
}

/// Record one Amazon ECS service of the inventory as owned by `account_id`, by the ARN its
/// record carries.
///
/// No call of this endpoint can do it: an ARN names the account that owns the resource, and
/// the ECS emulator mints every service ARN for the one account it serves — so the state a
/// cross-account inventory would have is seeded here, the way the AWS-reserved utilization
/// series is ([`publish`]), and read back over the wire like any client.
fn own_service_in_account(
    engine: &Arc<engine::InMemoryEngine>,
    cluster: &str,
    service: &str,
    account_id: &str,
) {
    use service_com_amazonaws_ecs_amazonec2containerservicev20141113::state as ecs;

    let inventory = engine.state::<ecs::ComAmazonawsEcsAmazonec2containerservicev20141113State>();
    let mut inventory = inventory.lock();
    let record = inventory
        .services
        .get_mut(&(cluster.to_string(), service.to_string()))
        .unwrap_or_else(|| panic!("the ECS store holds the service {service} of {cluster}"));
    record.arn = format!("arn:aws:ecs:{REGION}:{account_id}:service/{cluster}/{service}");
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

/// Register one Fargate-compatible task definition with the given task-level size,
/// returning its ARN.
async fn register_task_definition(
    router: &MultiServiceRouter,
    family: &str,
    cpu: &str,
    memory: &str,
    case: &str,
) -> String {
    let body = ecs_ok(
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
    body["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskDefinitionArn, got {body}"))
        .to_string()
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
            // The revision above is in the `awsvpc` network mode, whose tasks each get
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
/// utilization these rows analyse is seeded into the metric store the way ECS writes it.
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

/// The one recommendation a request for `arn` must answer with, asserting it reports no
/// error for it.
async fn only_recommendation(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        "GetECSServiceRecommendations",
        json!({ "serviceArns": [arn] }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert!(
        list(&body, "errors", case).is_empty(),
        "case {case:?}: expected no error entry, got {body}"
    );
    let found = list(&body, "ecsServiceRecommendations", case);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one recommendation, got {body}"
    );
    found[0].clone()
}

/// A list member of a response body.
fn list(body: &Value, member: &str, case: &str) -> Vec<Value> {
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list {member}, got {body}"))
        .clone()
}

/// The entry of a utilization-metric list for one metric name and statistic.
fn metric_entry(metrics: &[Value], name: &str, statistic: &str, case: &str) -> Value {
    metrics
        .iter()
        .find(|entry| {
            entry["name"].as_str() == Some(name) && entry["statistic"].as_str() == Some(statistic)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {name} {statistic} entry, got {metrics:?}")
        })
        .clone()
}

/// The `value` of one utilization metric.
fn metric_value(metrics: &[Value], name: &str, statistic: &str, case: &str) -> f64 {
    metric_entry(metrics, name, statistic, case)["value"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} {statistic} value"))
}

/// Compare two computed utilization values, which are quotients of observations and
/// capacities rather than exact decimals.
fn assert_close(actual: f64, expected: f64, case: &str, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "case {case:?}: expected {what} to be {expected}, got {actual}"
    );
}

/// What a month of `tasks` tasks of a `cpu`/`memory` Fargate task size costs, at the public
/// per-vCPU-hour and per-GB-hour prices: 1024 CPU units is one vCPU and 1024 MiB is one GB.
fn monthly_cost(cpu: f64, memory: f64, tasks: i64) -> f64 {
    let per_task_hour =
        cpu / 1024.0 * PRICE_PER_VCPU_HOUR + memory / 1024.0 * PRICE_PER_GB_HOUR;
    round2(tasks as f64 * per_task_hour * HOURS_PER_MONTH)
}

/// A money figure or a percentage, to whole cents.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// A container reservation of `reserved` out of a task of `current` capacity, at a task of
/// `recommended` capacity: the same share of the task in whole units, and never nothing.
fn scaled(reserved: f64, current: f64, recommended: f64) -> i64 {
    ((reserved * recommended / current).round() as i64).max(1)
}
