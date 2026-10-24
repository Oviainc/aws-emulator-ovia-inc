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
//! * a service the analysis has no input for — unknown, not on Fargate, or with no
//!   observation inside the window — is reported in `errors` rather than given an invented
//!   recommendation.
//!
//! Every row differs along what that rule generalises over: the observed utilization, the
//! current task size and the form it is registered in, the look-back period in force, and
//! which analysis input is missing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_cloudwatch, call_ecs, call_in_region, emulator_with_analysed_services, error_code,
    opt_in, REGION,
};

/// The account id every ARN the emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The two `AWS/ECS` series an ECS service recommendation is computed from.
const CPU_METRIC: &str = "CPUUtilization";
/// See [`CPU_METRIC`].
const MEMORY_METRIC: &str = "MemoryUtilization";

/// One row of the analysis table: a service's persisted size, what was observed for it, and
/// the recommendation that follows.
#[derive(Debug)]
struct AnalysisCase {
    name: &'static str,
    service: &'static str,
    /// The task definition's task-level `cpu`/`memory`, in the form it is registered with.
    cpu: &'static str,
    memory: &'static str,
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
    let router = emulator_with_analysed_services();
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
            case,
        )
        .await;
        publish(
            &router,
            cluster,
            test.service,
            CPU_METRIC,
            &test.cpu_values,
            published_at,
            case,
        )
        .await;
        publish(
            &router,
            cluster,
            test.service,
            MEMORY_METRIC,
            &test.memory_values,
            published_at,
            case,
        )
        .await;

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
        // A savings opportunity is a price difference and a container recommendation needs
        // per-container utilization; the emulator holds neither, so neither is invented.
        assert!(
            option["savingsOpportunity"].is_null()
                && option["savingsOpportunityAfterDiscounts"].is_null()
                && option["containerRecommendations"].is_null(),
            "case {case:?}: expected no invented savings or container recommendation, got {option}"
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

/// One row of the no-analysis table: which analysis input is missing, and the error code
/// the request is answered with for the service that misses it.
#[derive(Debug)]
struct ErrorCase {
    name: &'static str,
    service: &'static str,
    /// The ECS launch type the service is created with, or `None` to create no service.
    launch_type: Option<&'static str>,
    /// Which series to publish, and how long ago.
    publish: &'static [&'static str],
    age_secs: i64,
    code: &'static str,
}

#[tokio::test]
async fn a_service_with_no_analysis_input_is_reported_as_an_error() {
    let router = emulator_with_analysed_services();
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
            code: "ResourceNotFound",
        },
        ErrorCase {
            name: "the service does not run on Fargate",
            service: "oncontainerinstances",
            launch_type: Some("EC2"),
            publish: &[CPU_METRIC, MEMORY_METRIC],
            age_secs: 3_600,
            code: "UnsupportedResource",
        },
        ErrorCase {
            name: "nothing was ever published for the service",
            service: "unobserved",
            launch_type: Some("FARGATE"),
            publish: &[],
            age_secs: 3_600,
            code: "InsufficientMetrics",
        },
        ErrorCase {
            name: "only one of the two series was published",
            service: "cpuonly",
            launch_type: Some("FARGATE"),
            publish: &[CPU_METRIC],
            age_secs: 3_600,
            code: "InsufficientMetrics",
        },
        ErrorCase {
            name: "every observation is older than the look-back period",
            service: "stale",
            launch_type: Some("FARGATE"),
            publish: &[CPU_METRIC, MEMORY_METRIC],
            // 20 days back, which the default DAYS_14 window does not reach.
            age_secs: 20 * 86_400,
            code: "InsufficientMetrics",
        },
    ];

    for test in &cases {
        let case = test.name;
        let family = format!("{}family", test.service);
        if let Some(launch_type) = test.launch_type {
            register_task_definition(&router, &family, "512", "1024", case).await;
            create_service(&router, cluster, test.service, &family, launch_type, case).await;
        }
        for metric in test.publish {
            publish(
                &router,
                cluster,
                test.service,
                metric,
                &[50.0, 60.0],
                now_secs() - test.age_secs,
                case,
            )
            .await;
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
            Some(test.code),
            "case {case:?}: expected the code {}, got {body}",
            test.code
        );
        assert!(
            errors[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains(&arn)),
            "case {case:?}: expected a message naming {arn}, got {body}"
        );
    }

    // `maxResults` bounds the whole response: a page of services that have no analysis
    // carries at most that many error entries, plus a token for the rest.
    let unanalysable: Vec<String> = ["unobserved", "cpuonly", "stale"]
        .iter()
        .map(|service| service_arn(cluster, service))
        .collect();
    let (status, body) = call(
        &router,
        "GetECSServiceRecommendations",
        json!({ "serviceArns": unanalysable, "maxResults": 1 }),
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
    // the modelled 400, never with an empty page. A member the MODEL constrains (a filter
    // name outside `ECSServiceRecommendationFilterName`) is refused by the framework's own
    // constraint walk before the handler runs, with a `ValidationException`; everything the
    // model leaves open is refused by the handler with the operation's own
    // `InvalidParameterValueException`.
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
            "ValidationException",
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

    // A well-formed request for a service nothing holds is NOT a refusal: it is an error
    // entry about that one service.
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
    assert_eq!(
        list(&body, "errors", "unknown service").len(),
        1,
        "a well-formed ARN for an unknown service: expected one error entry, got {body}"
    );
}

#[tokio::test]
async fn recommendations_are_paged_in_arn_order_and_filtered_by_finding() {
    let router = emulator_with_analysed_services();
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
        create_service(&router, cluster, service, &family, "FARGATE", service).await;
        publish(
            &router,
            cluster,
            service,
            CPU_METRIC,
            &cpu_values,
            published_at,
            service,
        )
        .await;
        publish(
            &router,
            cluster,
            service,
            MEMORY_METRIC,
            &[20.0, 30.0],
            published_at,
            service,
        )
        .await;
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
    let router = emulator_with_analysed_services();
    opt_in(&router).await;
    let cluster = "regional";
    create_cluster(&router, cluster, "setup").await;
    let family = "regionalfamily";
    register_task_definition(&router, family, "512", "1024", "setup").await;
    let arn = create_service(&router, cluster, "checkout", family, "FARGATE", "setup").await;
    let published_at = now_secs() - 3_600;
    for metric in [CPU_METRIC, MEMORY_METRIC] {
        publish(
            &router,
            cluster,
            "checkout",
            metric,
            &[40.0, 50.0],
            published_at,
            "setup",
        )
        .await;
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
        // reported in the error channel exactly as an ARN naming nothing is.
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
        let errors = list(&body, "errors", &case);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected one error entry, got {body}"
        );
        assert_eq!(
            errors[0]["identifier"].as_str(),
            Some(foreign.as_str()),
            "case {case:?}: expected the error to name {foreign}, got {body}"
        );
        assert_eq!(
            errors[0]["code"].as_str(),
            Some("ResourceNotFound"),
            "case {case:?}: expected ResourceNotFound, got {body}"
        );
    }
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
    case: &str,
) -> String {
    let body = ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": service,
            "taskDefinition": family,
            "desiredCount": 1,
            "launchType": launch_type,
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

/// Publish `values` of one `AWS/ECS` series of one service, at `at`.
async fn publish(
    router: &MultiServiceRouter,
    cluster: &str,
    service: &str,
    metric_name: &str,
    values: &[f64],
    at: i64,
    case: &str,
) {
    let dimensions = json!([
        { "Name": "ClusterName", "Value": cluster },
        { "Name": "ServiceName", "Value": service },
    ]);
    let data: Vec<Value> = values
        .iter()
        .map(|value| {
            json!({
                "MetricName": metric_name,
                "Dimensions": dimensions,
                "Value": value,
                "Unit": "Percent",
                "Timestamp": at,
            })
        })
        .collect();
    let (status, body) = call_cloudwatch(
        router,
        "PutMetricData",
        json!({ "Namespace": "AWS/ECS", "MetricData": data }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutMetricData to answer 200, got {status}: {body}"
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
