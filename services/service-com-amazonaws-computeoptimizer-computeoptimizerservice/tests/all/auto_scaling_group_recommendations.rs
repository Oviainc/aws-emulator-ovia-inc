// SPDX-License-Identifier: CC0-1.0

//! `GetAutoScalingGroupRecommendations`, end to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests
//! prove is what a client sees. The EXPECTED values are computed here from the stored
//! analysis (`state::...State::default().auto_scaling_group_analyses`, the same rows the
//! running store holds) with the formulas written out, so a handler that reported a canned
//! figure instead of computing one would fail these assertions.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    AnalyzedAutoScalingGroup, ComAmazonawsComputeoptimizerComputeoptimizerserviceState as State,
    GroupConfiguration, ANALYSIS_DELAY, CPU_ARCHITECTURE_AWS_ARM64, CPU_ARCHITECTURE_CURRENT,
    CURRENCY_USD, HOURS_PER_MONTH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    auto_scaling_group_arn, call, call_in_region, emulator, error_code, opt_in, put_preference,
    ACCOUNT, REGION,
};

/// The analysis the running store holds: the same rows every emulator starts with.
fn analysis() -> Vec<AnalyzedAutoScalingGroup> {
    State::default().auto_scaling_group_analyses
}

/// Opt the account in and let the analysis its enrollment starts come due. The span is
/// the service's own ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// Store one preference for every analysed group.
///
/// "You can create recommendation preferences for Auto Scaling groups only at the resource
/// level by specifying a scope name of `ResourceArn` and a scope value of the Auto Scaling
/// group Amazon Resource Name" (`PutRecommendationPreferences$scope`), so a preference
/// that is to apply to every group is written once per group ARN.
async fn put_group_preference(router: &MultiServiceRouter, preference: &Value) {
    for group in analysis() {
        let mut request = preference.clone();
        request["resourceType"] = json!("AutoScalingGroup");
        request["scope"] = json!({"name": "ResourceArn", "value": group.arn_in(REGION)});
        put_preference(router, &request).await;
    }
}

/// Call the operation, assert it succeeded, and return the body.
async fn recommendations(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "GetAutoScalingGroupRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "request {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The reported names, in the order they were reported.
fn reported_names(body: &Value) -> Vec<String> {
    body["autoScalingGroupRecommendations"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| entry["autoScalingGroupName"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The one reported recommendation for `name`.
fn reported<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["autoScalingGroupRecommendations"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["autoScalingGroupName"] == name)
        })
        .unwrap_or_else(|| panic!("case {name:?}: no recommendation was reported: {body}"))
}

/// Two decimal places, the precision the service reports money and percentages at.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// A JSON number, or `f64::NAN` when the member is missing — which fails any comparison
/// and so reports as a mismatch rather than a panic.
fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

/// The estimated monthly on-demand cost of a configuration: one instance-hour × the
/// instances it runs × the hours in a month.
fn monthly_cost(configuration: &GroupConfiguration) -> f64 {
    round2(
        configuration.hourly_price
            * f64::from(configuration.desired_capacity)
            * HOURS_PER_MONTH,
    )
}

/// Nothing is reported until the account enrolls AND the analysis that enrollment starts
/// is due; a read never changes what the next caller sees.
#[tokio::test]
async fn recommendations_appear_only_once_the_account_is_enrolled_and_analysed() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "GetAutoScalingGroupRecommendations",
        json!({}),
    )
    .await;
    assert_eq!(
        status, 403,
        "not opted in: expected 403, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("OptInRequiredException"),
        "not opted in: expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;
    let fresh = recommendations(&router, json!({})).await;
    assert_eq!(
        reported_names(&fresh),
        Vec::<String>::new(),
        "just opted in: the analysis is not due yet, so nothing is analysed: {fresh}"
    );
    assert_eq!(
        fresh["errors"],
        json!([]),
        "just opted in: no identifier was asked about, so there is no error: {fresh}"
    );

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let analysed = recommendations(&router, json!({})).await;
    let expected: Vec<String> = analysis().iter().map(|group| group.name.clone()).collect();
    assert_eq!(
        reported_names(&analysed),
        expected,
        "analysis due: expected every analysed group in analysis order: {analysed}"
    );

    // A reporting operation must not change what a later caller sees.
    let again = recommendations(&router, json!({})).await;
    assert_eq!(
        again, analysed,
        "a second read must report the same analysis, got {again} after {analysed}"
    );
}

/// Every member of a recommendation is the stored observation or the figure computed from
/// it: the finding from the candidate configurations, each option's rank from its
/// position, its projected utilization from the observed utilization and the capacity it
/// is projected onto, and its savings from the stored on-demand prices.
#[tokio::test]
async fn each_group_reports_its_configuration_and_the_figures_derived_from_it() {
    let router = emulator();
    analysed(&router).await;
    let body = recommendations(&router, json!({})).await;

    for group in analysis() {
        let case = group.name.clone();
        let entry = reported(&body, &case);

        assert_eq!(
            entry["autoScalingGroupArn"],
            group.arn_in(REGION),
            "case {case:?}: expected the group's ARN in the Region the request addressed {}, \
             got {entry}",
            group.arn_in(REGION)
        );
        assert_eq!(
            entry["accountId"], group.account_id,
            "case {case:?}: expected the owning account {}, got {entry}",
            group.account_id
        );
        assert_eq!(
            entry["currentPerformanceRisk"], group.current_performance_risk,
            "case {case:?}: expected the stored current performance risk, got {entry}"
        );
        // The default look-back period, since no preference sets one: `DAYS_14`.
        assert!(
            (number(&entry["lookBackPeriodInDays"]) - 14.0).abs() < 0.01,
            "case {case:?}: expected the default 14-day analysis window, got {entry}"
        );

        // A group is NotOptimized exactly when the analysis found a candidate that is
        // not the group's own configuration.
        let differs = |candidate: &GroupConfiguration| {
            candidate.instance_types != group.current.instance_types
                || candidate.desired_capacity != group.current.desired_capacity
                || candidate.min_size != group.current.min_size
                || candidate.max_size != group.current.max_size
        };
        let expected_finding = if group.candidates.iter().any(differs) {
            "NotOptimized"
        } else {
            "Optimized"
        };
        assert_eq!(
            entry["finding"], expected_finding,
            "case {case:?}: expected finding {expected_finding}, got {entry}"
        );

        // The current configuration, reported the way the model splits a single-instance
        // -type group from a mixed one.
        let configuration = &entry["currentConfiguration"];
        assert_eq!(
            configuration["desiredCapacity"], group.current.desired_capacity,
            "case {case:?}: expected the stored desired capacity, got {configuration}"
        );
        assert_eq!(
            configuration["minSize"], group.current.min_size,
            "case {case:?}: expected the stored minimum size, got {configuration}"
        );
        assert_eq!(
            configuration["maxSize"], group.current.max_size,
            "case {case:?}: expected the stored maximum size, got {configuration}"
        );
        if group.current.instance_types.len() > 1 {
            assert_eq!(
                configuration["type"], "MixedInstanceTypes",
                "case {case:?}: a group with several instance types is mixed, got {configuration}"
            );
            assert_eq!(
                configuration["mixedInstanceTypes"],
                json!(group.current.instance_types),
                "case {case:?}: expected the stored instance types, got {configuration}"
            );
            assert_eq!(
                configuration["allocationStrategy"],
                json!(group.current.allocation_strategy),
                "case {case:?}: expected the stored allocation strategy, got {configuration}"
            );
        } else {
            assert_eq!(
                configuration["type"], "SingleInstanceType",
                "case {case:?}: one instance type is a single-instance-type group, got \
                 {configuration}"
            );
            assert_eq!(
                configuration["instanceType"], group.current.instance_types[0],
                "case {case:?}: expected the stored instance type, got {configuration}"
            );
            assert!(
                configuration["mixedInstanceTypes"].is_null(),
                "case {case:?}: a single-instance-type group lists no mixed types, got \
                 {configuration}"
            );
        }

        // The observed utilization, at the only statistic this API reports.
        let observed = entry["utilizationMetrics"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            observed.len(),
            group.utilization.len(),
            "case {case:?}: expected one metric per stored observation, got {entry}"
        );
        for (metric, (name, value)) in observed.iter().zip(&group.utilization) {
            assert_eq!(
                metric["name"], *name,
                "case {case:?}: expected metric {name}, got {metric}"
            );
            assert_eq!(
                metric["statistic"], "Maximum",
                "case {case:?}: the API reports only the Maximum statistic, got {metric}"
            );
            assert!(
                (number(&metric["value"]) - *value).abs() < 0.01,
                "case {case:?}: expected the observed {name} of {value}, got {metric}"
            );
        }

        // The options: one per candidate, in analysis order.
        let options = entry["recommendationOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            options.len(),
            group.candidates.len(),
            "case {case:?}: expected one option per stored candidate, got {entry}"
        );
        for (index, (option, candidate)) in options.iter().zip(&group.candidates).enumerate() {
            let case = format!("{case}#option{index}");
            assert_eq!(
                option["rank"],
                json!(index + 1),
                "case {case:?}: the top option ranks 1, got {option}"
            );
            assert!(
                (number(&option["performanceRisk"]) - candidate.performance_risk).abs() < 0.01,
                "case {case:?}: expected the stored performance risk {}, got {option}",
                candidate.performance_risk
            );

            // Savings: the monthly cost the candidate removes, and that saving as a
            // percentage of the group's current monthly cost.
            let current_cost = monthly_cost(&group.current);
            let expected_saving = round2((current_cost - monthly_cost(candidate)).max(0.0));
            let expected_percentage = if current_cost == 0.0 {
                0.0
            } else {
                round2(expected_saving / current_cost * 100.0)
            };
            let savings = &option["savingsOpportunity"];
            assert!(
                (number(&savings["estimatedMonthlySavings"]["value"]) - expected_saving).abs()
                    < 0.01,
                "case {case:?}: expected a monthly saving of {expected_saving}, got {savings}"
            );
            assert_eq!(
                savings["estimatedMonthlySavings"]["currency"], CURRENCY_USD,
                "case {case:?}: expected the saving in {CURRENCY_USD}, got {savings}"
            );
            assert!(
                (number(&savings["savingsOpportunityPercentage"]) - expected_percentage).abs()
                    < 0.01,
                "case {case:?}: expected a saving of {expected_percentage}% of the current \
                 monthly cost, got {savings}"
            );
            assert!(
                option["savingsOpportunityAfterDiscounts"].is_null(),
                "case {case:?}: no savings-estimation-mode preference is set, so no \
                 after-discount saving is reported, got {option}"
            );

            // Projected utilization: the same work on the candidate's capacity, capped
            // at 100 because both metrics are percentages.
            let projected = option["projectedUtilizationMetrics"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for (metric, (name, value)) in projected.iter().zip(&group.utilization) {
                let (from, to) = if name == "Memory" {
                    (
                        group.current.memory_gib * f64::from(group.current.desired_capacity),
                        candidate.memory_gib * f64::from(candidate.desired_capacity),
                    )
                } else {
                    (
                        group.current.vcpus * f64::from(group.current.desired_capacity),
                        candidate.vcpus * f64::from(candidate.desired_capacity),
                    )
                };
                let expected = round2((value * from / to).min(100.0));
                assert_eq!(
                    metric["statistic"], "Maximum",
                    "case {case:?}: the API reports only the Maximum statistic, got {metric}"
                );
                assert!(
                    (number(&metric["value"]) - expected).abs() < 0.01,
                    "case {case:?}: expected a projected {name} of {expected}, got {metric}"
                );
            }
        }

        // The preferences the recommendation was generated under: the service's
        // documented defaults, since this account stored none.
        let preferences = &entry["effectiveRecommendationPreferences"];
        assert_eq!(
            preferences["lookBackPeriod"], "DAYS_14",
            "case {case:?}: expected the default look-back period, got {preferences}"
        );
        assert_eq!(
            preferences["inferredWorkloadTypes"], "Active",
            "case {case:?}: the inferred workload type feature is active by default, got \
             {preferences}"
        );
        assert_eq!(
            preferences["savingsEstimationMode"]["source"], "PublicPricing",
            "case {case:?}: the figures come from the stored public prices, got {preferences}"
        );
        assert!(
            preferences["preferredResources"]
                .as_array()
                .is_some_and(|resources| !resources.is_empty()),
            "case {case:?}: expected the default preferred resources, got {preferences}"
        );
        assert_eq!(
            entry["inferredWorkloadTypes"],
            json!(group.inferred_workload_types),
            "case {case:?}: expected the inferred workload types of the analysis, got {entry}"
        );
    }
}

/// `cpuVendorArchitectures` selects which candidates are reported — "when you specify
/// `AWS_ARM64` ... Compute Optimizer returns recommendations that consist of Graviton
/// instance types only" — and the effective preference reports the architectures the
/// reported options consist of. A value outside the enum is refused.
#[tokio::test]
async fn the_cpu_vendor_architecture_preference_selects_the_options_reported() {
    let router = emulator();
    analysed(&router).await;

    // The rows are the model's `CpuVendorArchitecture` enum
    // (`crate::ops::types::CPU_VENDOR_ARCHITECTURE_VALUES`): no preference, each value on
    // its own, and both together.
    let cases: [Option<Vec<&str>>; 4] = [
        None,
        Some(vec![CPU_ARCHITECTURE_CURRENT]),
        Some(vec![CPU_ARCHITECTURE_AWS_ARM64]),
        Some(vec![CPU_ARCHITECTURE_CURRENT, CPU_ARCHITECTURE_AWS_ARM64]),
    ];
    for wanted in cases {
        let case = format!("cpuVendorArchitectures={wanted:?}");
        let mut request = json!({});
        if let Some(values) = &wanted {
            request["recommendationPreferences"] = json!({"cpuVendorArchitectures": values});
        }
        let body = recommendations(&router, request).await;

        for group in analysis() {
            let case = format!("{case} group={}", group.name);
            let entry = reported(&body, &group.name);
            let expected: Vec<String> = group
                .candidates
                .iter()
                .filter(|candidate| {
                    wanted
                        .as_ref()
                        .is_none_or(|values| values.contains(&candidate.architecture.as_str()))
                })
                .flat_map(|candidate| candidate.instance_types.clone())
                .collect();
            let reported_types: Vec<String> = entry["recommendationOptions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .flat_map(|option| {
                    let configuration = &option["configuration"];
                    configuration["mixedInstanceTypes"]
                        .as_array()
                        .map(|types| {
                            types
                                .iter()
                                .map(|value| value.as_str().unwrap_or("").to_string())
                                .collect::<Vec<String>>()
                        })
                        .unwrap_or_else(|| {
                            vec![configuration["instanceType"]
                                .as_str()
                                .unwrap_or("")
                                .to_string()]
                        })
                })
                .collect();
            assert_eq!(
                reported_types, expected,
                "case {case:?}: expected the options of the admitted architectures, got {entry}"
            );

            // The effective preference reports what the reported options consist of.
            let mut architectures: Vec<String> = Vec::new();
            for candidate in &group.candidates {
                let admitted = wanted
                    .as_ref()
                    .is_none_or(|values| values.contains(&candidate.architecture.as_str()));
                if admitted && !architectures.contains(&candidate.architecture) {
                    architectures.push(candidate.architecture.clone());
                }
            }
            assert_eq!(
                entry["effectiveRecommendationPreferences"]["cpuVendorArchitectures"],
                json!(architectures),
                "case {case:?}: expected the architectures of the reported options, got {entry}"
            );
        }
    }

    let (status, body) = call(
        &router,
        "GetAutoScalingGroupRecommendations",
        json!({"recommendationPreferences": {"cpuVendorArchitectures": ["AWS_ARM32"]}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a CPU vendor architecture outside the enum must be refused, got {status}: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("AWS_ARM32")),
        "a CPU vendor architecture outside the enum: the refusal must name the value, got {body}"
    );
}

/// `migrationEffort` follows the rule the model states: `VeryLow` for a candidate of the
/// same CPU architecture, `Low` for a Graviton candidate where a workload type was
/// inferred, `Medium` for a Graviton candidate where none was — which is also what a group
/// reports once the account deactivates the workload-type inference.
#[tokio::test]
async fn migration_effort_follows_the_architecture_and_inferred_workload_rule() {
    for inference_active in [true, false] {
        let router = emulator();
        analysed(&router).await;
        if !inference_active {
            put_group_preference(&router, &json!({"inferredWorkloadTypes": "Inactive"})).await;
        }
        let body = recommendations(&router, json!({})).await;

        for group in analysis() {
            let entry = reported(&body, &group.name);
            let options = entry["recommendationOptions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for (option, candidate) in options.iter().zip(&group.candidates) {
                let case = format!(
                    "group={} architecture={} inference={inference_active}",
                    group.name, candidate.architecture
                );
                let expected = if candidate.architecture != CPU_ARCHITECTURE_AWS_ARM64 {
                    "VeryLow"
                } else if inference_active && !group.inferred_workload_types.is_empty() {
                    "Low"
                } else {
                    "Medium"
                };
                assert_eq!(
                    option["migrationEffort"], expected,
                    "case {case:?}: expected migration effort {expected}, got {option}"
                );
            }

            let expected_inferred = if inference_active {
                json!(group.inferred_workload_types)
            } else {
                Value::Null
            };
            assert_eq!(
                entry["inferredWorkloadTypes"], expected_inferred,
                "case {:?}: inference={inference_active} decides whether workload types are \
                 reported, got {entry}",
                group.name
            );
        }
    }
}

/// A filter selects by the value the recommendation carries: its finding, the resource
/// type this operation reports, or a workload type the analysis inferred. A well-formed
/// filter no Auto Scaling group recommendation can satisfy selects nothing, and a filter
/// outside the model's vocabulary is refused.
#[tokio::test]
async fn filters_select_by_finding_source_type_and_inferred_workload() {
    let router = emulator();
    analysed(&router).await;

    let groups = analysis();
    let optimized: Vec<String> = groups
        .iter()
        .filter(|group| {
            !group.candidates.iter().any(|candidate| {
                candidate.instance_types != group.current.instance_types
                    || candidate.desired_capacity != group.current.desired_capacity
            })
        })
        .map(|group| group.name.clone())
        .collect();
    let not_optimized: Vec<String> = groups
        .iter()
        .map(|group| group.name.clone())
        .filter(|name| !optimized.contains(name))
        .collect();
    let every_name: Vec<String> = groups.iter().map(|group| group.name.clone()).collect();
    let with_emr: Vec<String> = groups
        .iter()
        .filter(|group| group.inferred_workload_types.iter().any(|kind| kind == "AmazonEmr"))
        .map(|group| group.name.clone())
        .collect();

    // The rows span every `FilterName` the model declares
    // (`crate::ops::types::FILTER_NAME_VALUES`), including values that are valid for the
    // filter but that no Auto Scaling group recommendation can carry.
    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("Finding", vec!["Optimized"], optimized.clone()),
        ("Finding", vec!["NotOptimized"], not_optimized.clone()),
        (
            "Finding",
            vec!["Optimized", "NotOptimized"],
            every_name.clone(),
        ),
        // An EC2 instance finding: well formed, and no group carries it.
        ("Finding", vec!["Underprovisioned"], Vec::new()),
        (
            "RecommendationSourceType",
            vec!["AutoScalingGroup"],
            every_name.clone(),
        ),
        ("RecommendationSourceType", vec!["Ec2Instance"], Vec::new()),
        ("InferredWorkloadTypes", vec!["AmazonEmr"], with_emr),
        ("InferredWorkloadTypes", vec!["SQLServer"], Vec::new()),
        // An Auto Scaling group recommendation reports no finding reason codes.
        ("FindingReasonCodes", vec!["CPUOverprovisioned"], Vec::new()),
    ];
    for (name, values, expected) in cases {
        let case = format!("filter {name}={values:?}");
        let body = recommendations(
            &router,
            json!({"filters": [{"name": name, "values": values}]}),
        )
        .await;
        assert_eq!(
            reported_names(&body),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
    }

    // Two filters are an AND: a group must satisfy both.
    let body = recommendations(
        &router,
        json!({"filters": [
            {"name": "Finding", "values": ["Optimized"]},
            {"name": "InferredWorkloadTypes", "values": ["AmazonEmr"]},
        ]}),
    )
    .await;
    assert_eq!(
        reported_names(&body),
        Vec::<String>::new(),
        "two filters are an AND: no group is both optimized and an EMR workload, got {body}"
    );

    for (case, filter) in [
        ("an unknown filter name", json!({"name": "Region", "values": ["us-east-1"]})),
        (
            "a value outside the filter's vocabulary",
            json!({"name": "Finding", "values": ["Rightsized"]}),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetAutoScalingGroupRecommendations",
            json!({"filters": [filter]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
    }
}

/// `maxResults` sizes the page and the `nextToken` walks the rest: every analysed group is
/// reported exactly once across the pages, the last page carries no token, and a token
/// this service did not issue is refused.
#[tokio::test]
async fn pagination_reports_every_group_exactly_once() {
    let router = emulator();
    analysed(&router).await;

    let every_name: Vec<String> = analysis().iter().map(|group| group.name.clone()).collect();
    // The rows are page sizes around the number of analysed groups: one per page, a page
    // that holds every group, and the ceiling the model puts on `maxResults`.
    for page_size in [1_usize, 2, every_name.len(), 1_000] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let body = recommendations(&router, request).await;
            let names = reported_names(&body);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page may hold at most {page_size} groups, got {body}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= every_name.len() + 1,
                "case {case:?}: paging must terminate, got {seen:?}"
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, every_name,
            "case {case:?}: expected every analysed group exactly once, got {seen:?}"
        );
    }

    let (status, body) = call(
        &router,
        "GetAutoScalingGroupRecommendations",
        json!({"nextToken": "not-a-token-this-service-issued"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a forged nextToken must be refused, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("InvalidParameterValueException"),
        "a forged nextToken: expected InvalidParameterValueException, got {body}"
    );
}

/// `autoScalingGroupArns` and `accountIds` select what is reported; an identifier with no
/// recommendation is absent from the answer, and a malformed one is refused.
#[tokio::test]
async fn requested_identifiers_are_selected_omitted_or_refused() {
    let router = emulator();
    analysed(&router).await;

    let groups = analysis();
    let known = groups[0].clone();
    let unknown = auto_scaling_group_arn("not-analysed-asg");

    // One known ARN: that group, and no error.
    let body =
        recommendations(&router, json!({"autoScalingGroupArns": [known.arn_in(REGION)]})).await;
    assert_eq!(
        reported_names(&body),
        vec![known.name.clone()],
        "one known ARN: expected exactly that group, got {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "one known ARN: expected no error, got {body}"
    );

    // A known ARN beside an unanalysed one: the group, and no error — an identifier with no
    // recommendation is absent from the answer rather than reported as a failure.
    let body = recommendations(
        &router,
        json!({"autoScalingGroupArns": [known.arn_in(REGION), unknown.clone()]}),
    )
    .await;
    assert_eq!(
        reported_names(&body),
        vec![known.name.clone()],
        "a known and an unanalysed ARN: expected only the analysed group, got {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "a known and an unanalysed ARN: expected no error, got {body}"
    );

    // `accountIds`: the emulator's own account holds the analysis; any OTHER account is a
    // member account of an organization this service records none of, so naming it is
    // refused rather than answered with an empty list — and more than one account id, or a
    // malformed one, is refused too. The whole class is covered in
    // `tests/all/cross_account_requests.rs`; these rows pin it for THIS reader.
    let own = ::engine::emulator_account_id().to_string();
    let every_name: Vec<String> = groups.iter().map(|group| group.name.clone()).collect();
    let cases: Vec<(Vec<&str>, Option<Vec<String>>)> = vec![
        (vec![own.as_str()], Some(every_name)),
        (vec!["111122223333"], None),
        (vec![own.as_str(), "111122223333"], None),
        (vec!["1234"], None),
        (vec!["00000000000a"], None),
    ];
    for (account_ids, expected) in cases {
        let case = format!("accountIds={account_ids:?}");
        let (status, body) = call(
            &router,
            "GetAutoScalingGroupRecommendations",
            json!({"accountIds": account_ids}),
        )
        .await;
        match expected {
            Some(names) => {
                assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
                assert_eq!(
                    reported_names(&body),
                    names,
                    "case {case:?}: expected exactly {names:?}, got {body}"
                );
            }
            None => {
                assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
                assert!(
                    error_code(&body).contains("InvalidParameterValueException"),
                    "case {case:?}: expected InvalidParameterValueException, got {body}"
                );
            }
        }
    }

    // An ARN that is not an Auto Scaling group's cannot identify a group to report on.
    for malformed in [
        "arn:aws:ec2:us-east-1:111122223333:instance/i-1234567890abcdef0",
        "web-tier-asg",
        "arn:aws:autoscaling:us-east-1:111122223333:launchConfiguration:web",
    ] {
        let (status, body) = call(
            &router,
            "GetAutoScalingGroupRecommendations",
            json!({"autoScalingGroupArns": [malformed]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {malformed:?}: expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {malformed:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// The look-back period in force for the group is the window the recommendation reports
/// it analysed: the stored preference where the account set one, and the documented
/// default (`DAYS_14`) where it did not.
#[tokio::test]
async fn the_look_back_period_preference_sets_the_reported_analysis_window() {
    // The rows are the model's `LookBackPeriodPreference` enum
    // (`crate::ops::types::LOOK_BACK_PERIOD_PREFERENCE_VALUES`) plus the absent case.
    let cases: [(Option<&str>, f64); 4] = [
        (None, 14.0),
        (Some("DAYS_14"), 14.0),
        (Some("DAYS_32"), 32.0),
        (Some("DAYS_93"), 93.0),
    ];
    for (preference, expected_days) in cases {
        let case = format!("lookBackPeriod={preference:?}");
        let router = emulator();
        analysed(&router).await;
        if let Some(preference) = preference {
            put_group_preference(&router, &json!({"lookBackPeriod": preference})).await;
        }

        let body = recommendations(&router, json!({})).await;
        for group in analysis() {
            let entry = reported(&body, &group.name);
            assert!(
                (number(&entry["lookBackPeriodInDays"]) - expected_days).abs() < 0.01,
                "case {case:?} group={:?}: expected a {expected_days}-day window, got {entry}",
                group.name
            );
            assert_eq!(
                entry["effectiveRecommendationPreferences"]["lookBackPeriod"],
                json!(preference.unwrap_or("DAYS_14")),
                "case {case:?} group={:?}: the effective preference must agree with the \
                 window, got {entry}",
                group.name
            );
        }
    }
}

/// The after-discount savings opportunity is reported only once the account activates the
/// savings-estimation-mode preference, and it is the on-demand saving net of the
/// Savings Plans discount that applies to the group's spend.
#[tokio::test]
async fn after_discount_savings_are_reported_only_when_the_preference_activates_them() {
    let router = emulator();
    analysed(&router).await;
    put_group_preference(&router, &json!({"savingsEstimationMode": "AfterDiscounts"})).await;
    let body = recommendations(&router, json!({})).await;

    for group in analysis() {
        let entry = reported(&body, &group.name);
        let options = entry["recommendationOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (option, candidate) in options.iter().zip(&group.candidates) {
            let case = format!(
                "group={} discount={}",
                group.name, group.savings_plans_discount_rate
            );
            let retained = 1.0 - group.savings_plans_discount_rate;
            let current = round2(monthly_cost(&group.current) * retained);
            let projected = round2(monthly_cost(candidate) * retained);
            let expected_saving = round2((current - projected).max(0.0));
            let expected_percentage = if current == 0.0 {
                0.0
            } else {
                round2(expected_saving / current * 100.0)
            };
            let after = &option["savingsOpportunityAfterDiscounts"];
            assert!(
                (number(&after["estimatedMonthlySavings"]["value"]) - expected_saving).abs()
                    < 0.01,
                "case {case:?}: expected an after-discount saving of {expected_saving}, got \
                 {after}"
            );
            assert!(
                (number(&after["savingsOpportunityPercentage"]) - expected_percentage).abs()
                    < 0.01,
                "case {case:?}: expected {expected_percentage}% after discounts, got {after}"
            );
            assert_eq!(
                after["estimatedMonthlySavings"]["currency"], CURRENCY_USD,
                "case {case:?}: expected the saving in {CURRENCY_USD}, got {after}"
            );

            // A discount only lowers the amount: with no discount against the group's
            // spend the two figures agree, and with one the after-discount saving is
            // strictly smaller.
            let on_demand = number(&option["savingsOpportunity"]["estimatedMonthlySavings"]["value"]);
            if group.savings_plans_discount_rate == 0.0 {
                assert!(
                    (on_demand - expected_saving).abs() < 0.01,
                    "case {case:?}: with no discount both figures agree, got {option}"
                );
            } else if on_demand > 0.0 {
                assert!(
                    expected_saving < on_demand,
                    "case {case:?}: a discounted saving must be smaller than the on-demand \
                     one ({on_demand}), got {expected_saving}"
                );
            }
        }
    }
}

/// ONE analysis of one group, read two ways: the account's Auto Scaling group SUMMARY
/// counts exactly the groups this operation reports, at the findings it reports for them,
/// and totals the saving of each group's top-ranked option — because the summary's rows
/// are projected from the same analysis rather than stated a second time.
#[tokio::test]
async fn the_auto_scaling_group_summary_agrees_with_the_recommendations() {
    let router = emulator();
    analysed(&router).await;
    let body = recommendations(&router, json!({})).await;
    let reported = body["autoScalingGroupRecommendations"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // What the recommendations say: how many groups carry each finding, and what adopting
    // the top-ranked option of each would save per month.
    let mut expected_counts: std::collections::BTreeMap<String, f64> =
        std::collections::BTreeMap::new();
    let mut expected_savings = 0.0;
    for entry in &reported {
        let finding = entry["finding"].as_str().unwrap_or_default().to_string();
        *expected_counts.entry(finding).or_default() += 1.0;
        let top = entry["recommendationOptions"]
            .as_array()
            .and_then(|options| options.iter().find(|option| option["rank"] == json!(1)));
        expected_savings += top.map_or(0.0, |option| {
            number(&option["savingsOpportunity"]["estimatedMonthlySavings"]["value"])
        });
    }

    let (status, summaries) = call(
        &router,
        "GetRecommendationSummaries",
        json!({"accountIds": [::engine::emulator_account_id()]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "summaries: expected 200, got {status}: {summaries}"
    );
    let summary = summaries["recommendationSummaries"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["recommendationResourceType"] == "AutoScalingGroup")
        })
        .unwrap_or_else(|| panic!("no Auto Scaling group summary was reported: {summaries}"));

    // The summary reports a count at every finding of the resource type, including the
    // ones no group carries, so a finding the recommendations do not show counts zero. The
    // roll-up names the `Finding` vocabulary in the instance surface's upper-snake spelling
    // (`NOT_OPTIMIZED`) while this reader reports the modelled enum value (`NotOptimized`):
    // the same classification, named as each surface names it.
    for finding in summary["summaries"].as_array().cloned().unwrap_or_default() {
        let name = finding["name"].as_str().unwrap_or_default().to_string();
        let reported_as = match name.as_str() {
            "NOT_OPTIMIZED" => "NotOptimized",
            "OPTIMIZED" => "Optimized",
            other => other,
        };
        let expected = expected_counts.get(reported_as).copied().unwrap_or(0.0);
        assert!(
            (number(&finding["value"]) - expected).abs() < 0.01,
            "case {name:?}: the summary must count the groups this operation reports \
             ({expected}), got {finding}"
        );
    }
    let counted: f64 = summary["summaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|finding| number(&finding["value"]))
        .sum();
    assert!(
        (counted - reported.len() as f64).abs() < 0.01,
        "the summary must count every reported group exactly once ({}), got {summary}",
        reported.len()
    );
    assert!(
        (number(&summary["savingsOpportunity"]["estimatedMonthlySavings"]["value"])
            - expected_savings)
            .abs()
            < 0.01,
        "the summary must total the top-ranked option of each group ({expected_savings}), \
         got {summary}"
    );
}

/// Compute Optimizer is a regional service: the endpoint a request addresses is the Region
/// whose resources it answers for, so the account's groups are reported at EVERY Region's
/// endpoint, each under that Region's own ARN — and an `autoScalingGroupArns` entry selects a
/// group exactly at the endpoint of the Region its ARN names.
///
/// The Regions below are rows of the same rule, not a catalog: any Region a client can sign
/// for behaves this way, which is why the expected ARN is computed per row from the stored
/// analysis rather than written out.
#[tokio::test]
async fn the_arns_reported_name_the_region_the_request_addressed() {
    let router = emulator();
    analysed(&router).await;

    let regions = ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"];
    let mut identities_per_region: Vec<Vec<String>> = Vec::new();
    for region in regions {
        let (status, body) = call_in_region(
            &router,
            region,
            "GetAutoScalingGroupRecommendations",
            json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: expected 200, got {status}: {body}"
        );
        let prefix = format!("arn:aws:autoscaling:{region}:{ACCOUNT}:");
        let mut identities: Vec<String> = Vec::new();
        for group in analysis() {
            let case = format!("{region}/{}", group.name);
            let entry = reported(&body, &group.name);
            let arn = entry["autoScalingGroupArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                arn,
                group.arn_in(region),
                "case {case:?}: expected the group's ARN at this endpoint {}, got {arn:?}",
                group.arn_in(region)
            );
            let identity = arn.strip_prefix(&prefix).map(str::to_string);
            identities.push(identity.unwrap_or_else(|| {
                panic!("case {case:?}: expected an ARN under {prefix}, got {arn:?}")
            }));
        }
        assert!(
            !identities.is_empty(),
            "case {region}: the Region's analysis is reported: {body}"
        );
        identities_per_region.push(identities);
    }
    for (region, identities) in regions.iter().zip(&identities_per_region) {
        assert_eq!(
            identities, &identities_per_region[0],
            "case {region}: the same groups are reported in every Region, each under its own \
             ARN, got {identities:?}"
        );
    }

    // (the endpoint the request addresses, the Region whose ARN it is asked about instead)
    let rows = [
        ("us-west-2", "us-east-1"),
        ("eu-central-1", "us-west-2"),
        ("us-east-1", "ap-southeast-2"),
    ];
    let group = analysis()
        .into_iter()
        .next()
        .expect("the analysis holds at least one group");
    for (region, other) in rows {
        // This endpoint's own ARN of the group selects it.
        let case = format!("{region} asked about its own ARN");
        let (status, body) = call_in_region(
            &router,
            region,
            "GetAutoScalingGroupRecommendations",
            json!({"autoScalingGroupArns": [group.arn_in(region)]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported_names(&body),
            vec![group.name.clone()],
            "case {case:?}: expected exactly that group, got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: expected no error, got {body}"
        );

        // Another Region's ARN of the same group names no group this endpoint analyses, so
        // it selects nothing and is simply absent from the answer.
        let case = format!("{region} asked about the {other} ARN");
        let foreign = group.arn_in(other);
        let (status, body) = call_in_region(
            &router,
            region,
            "GetAutoScalingGroupRecommendations",
            json!({"autoScalingGroupArns": [foreign.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported_names(&body),
            Vec::<String>::new(),
            "case {case:?}: an ARN of another Region selects nothing here, got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: an identifier with no recommendation is no error of the request, \
             got {body}"
        );
    }
}
