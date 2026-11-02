// SPDX-License-Identifier: CC0-1.0

//! `GetEC2InstanceRecommendations` over the wire: the recommendation this service derives
//! from each analysed Amazon EC2 instance, and how a request narrows, pages and refuses.
//!
//! The RULE under test: for every instance of the CALLER's own account the endpoint's Region
//! has analysed, the read reports a recommendation computed from the stored analysis — the
//! type it runs, the finding and reason codes the analysis reached, the utilization it
//! observed, and one ranked option per candidate instance type carrying the utilization
//! projected onto that type and the saving adopting it offers — narrowed by `accountIds`,
//! `instanceArns`, `filters` and `recommendationPreferences`, one page at a time; an
//! identifier the analysis holds no recommendation for is absent from the answer, and a
//! request the service cannot honour is refused with the operation's modelled 400.
//!
//! The analysis of an observed workload is what produces a recommendation, and no operation
//! of THIS API creates one, so the fixtures below record instances into the store the way
//! Compute Optimizer's own analysis would and then read them back only over the wire. What is
//! asserted is the rule over a TABLE of instances that differ in what the analysis found (a
//! type to size down to, one to size up to, nothing to move to), in how many candidates they
//! have and of which CPU architecture, and in whether they carry tags — never one recorded
//! call. Every money figure and every projected percentage is recomputed here from the stored
//! prices and capacities, so a response that is not derived from the store fails.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, InstanceConfiguration, ObservedInstance, ObservedInstanceMetric,
    ANALYSIS_DELAY, CPU_ARCHITECTURE_AWS_ARM64, CPU_ARCHITECTURE_CURRENT, DEFAULT_PAGE_SIZE,
    FINDING_OPTIMIZED, FINDING_OVERPROVISIONED, FINDING_UNDERPROVISIONED, HOURS_PER_MONTH,
    INSTANCE_STATE_RUNNING, MAX_REQUESTED_IDENTIFIERS, NAME_TAG_KEY, PERFORMANCE_RISK_VERY_LOW,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in_region, emulator_with_engine, error_code, opt_in, ACCOUNT};

/// The region the harness signs for, and so the regional endpoint every call below addresses
/// unless it names another.
const REGION: &str = "us-east-1";

/// The one `MetricStatistic` this API reports: "The Compute Optimizer API, Command Line
/// Interface (CLI), and SDKs return utilization metrics using only the `Maximum` statistic"
/// (`UtilizationMetric$statistic`).
const MAXIMUM: &str = "Maximum";

/// The other statistic the analysis records, which no reader of this API reports.
const AVERAGE: &str = "Average";

/// The ids of the analysed instances [`corpus`] records.
const SIZED_DOWN: &str = "i-0c1b2c3d4e5f80001";
const SIZED_UP: &str = "i-0c1b2c3d4e5f80002";
const ALREADY_RIGHT_SIZED: &str = "i-0c1b2c3d4e5f80003";
const TWO_ARCHITECTURES: &str = "i-0c1b2c3d4e5f80004";

/// One instance type as the analysis records it: the type, the vCPUs and memory (GiB) it
/// provides, its on-demand price per hour and its `CpuVendorArchitecture`.
///
/// The vCPU and memory figures are those of the named instance types in the Amazon EC2
/// instance-type documentation; the prices are their public on-demand Linux prices in
/// `us-east-1`.
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

fn m5_large() -> InstanceConfiguration {
    configuration("m5.large", 2.0, 8.0, 0.096, CPU_ARCHITECTURE_CURRENT)
}

fn m5_xlarge() -> InstanceConfiguration {
    configuration("m5.xlarge", 4.0, 16.0, 0.192, CPU_ARCHITECTURE_CURRENT)
}

fn m5_2xlarge() -> InstanceConfiguration {
    configuration("m5.2xlarge", 8.0, 32.0, 0.384, CPU_ARCHITECTURE_CURRENT)
}

fn m6g_xlarge() -> InstanceConfiguration {
    configuration("m6g.xlarge", 4.0, 16.0, 0.154, CPU_ARCHITECTURE_AWS_ARM64)
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
    finding_reason_codes: &[&str],
    inferred_workload_types: &[&str],
    tags: &[(&str, &str)],
) -> ObservedInstance {
    ObservedInstance {
        instance_id: instance_id.to_string(),
        account_id: ACCOUNT.to_string(),
        current,
        candidates,
        utilization,
        finding_reason_codes: finding_reason_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect(),
        current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
        inferred_workload_types: inferred_workload_types
            .iter()
            .map(|workload| (*workload).to_string())
            .collect(),
        instance_state: INSTANCE_STATE_RUNNING.to_string(),
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

/// The analysis output every fixture below reads: instances spanning the cases an instance
/// recommendation distinguishes — one the analysis would size DOWN (so adopting its option
/// saves), one it would size UP (so its option saves nothing), one it would leave as it is
/// (no option at all), and one with candidates of two CPU vendor architectures (so a CPU
/// vendor architecture preference has something to select). Two carry tags, so a name is
/// reported for them and none for the others.
fn corpus() -> Vec<ObservedInstance> {
    vec![
        instance(
            SIZED_DOWN,
            m5_2xlarge(),
            vec![m5_xlarge()],
            vec![
                metric("Cpu", MAXIMUM, 22.5),
                metric("Memory", MAXIMUM, 31.0),
                // A metric the instance type does not size: the workload's own traffic, in
                // bytes per second, which no option projects.
                metric("NETWORK_IN_BYTES_PER_SECOND", MAXIMUM, 1_250_000.0),
                // Observed under the other statistic, which this API does not report.
                metric("Cpu", AVERAGE, 12.0),
            ],
            &["MemoryOverprovisioned"],
            &["PostgreSql"],
            &[(NAME_TAG_KEY, "reporting-db"), ("Owner", "TeamA")],
        ),
        instance(
            SIZED_UP,
            m5_large(),
            vec![m5_xlarge()],
            vec![metric("Cpu", MAXIMUM, 88.0), metric("Memory", MAXIMUM, 97.0)],
            &["MemoryUnderprovisioned"],
            &["Redis"],
            &[(NAME_TAG_KEY, "session-cache")],
        ),
        instance(
            ALREADY_RIGHT_SIZED,
            m5_xlarge(),
            Vec::new(),
            vec![metric("Cpu", MAXIMUM, 55.0), metric("Memory", MAXIMUM, 60.0)],
            &[],
            &[],
            &[],
        ),
        instance(
            TWO_ARCHITECTURES,
            m5_2xlarge(),
            vec![m5_xlarge(), m6g_xlarge()],
            vec![metric("Cpu", MAXIMUM, 24.0), metric("Memory", MAXIMUM, 36.0)],
            &["CPUOverprovisioned"],
            &["AmazonEmr"],
            &[],
        ),
    ]
}

/// The analysed instance the corpus holds under `instance_id`.
fn analysed_instance(instance_id: &str) -> ObservedInstance {
    corpus()
        .into_iter()
        .find(|instance| instance.instance_id == instance_id)
        .unwrap_or_else(|| panic!("the corpus holds no instance {instance_id}"))
}

/// One analysed instance's ARN at the endpoint of `region`.
fn instance_arn_in(region: &str, instance_id: &str) -> String {
    format!("arn:aws:ec2:{region}:{ACCOUNT}:instance/{instance_id}")
}

/// One analysed instance's ARN at the endpoint every fixture addresses.
fn instance_arn(instance_id: &str) -> String {
    instance_arn_in(REGION, instance_id)
}

/// Record `instances` into the analysis this reader reports from, the way Compute Optimizer's
/// own analysis of an observed workload would. The instances this emulator ships with are
/// replaced, so the table below is exactly what the fixtures read.
fn record(engine: &Arc<engine::InMemoryEngine>, instances: Vec<ObservedInstance>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    guard.instances.clear();
    for instance in instances {
        guard.instances.insert(instance.instance_id.clone(), instance);
    }
}

/// The analysis and the preferences as the store holds them, so a test can prove a read moved
/// none of it.
fn snapshot(engine: &Arc<engine::InMemoryEngine>) -> String {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let guard = state.lock();
    format!("{:?} {:?}", guard.instances, guard.preferences)
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

/// Read instance recommendations, asserting the call was served.
async fn recommendations(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, "GetEC2InstanceRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// Assert a request is refused with the operation's modelled 400, and answer the message.
async fn refused(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let (status, body) = call(router, "GetEC2InstanceRecommendations", request.clone()).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {request} to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValueException",
        "case {case:?}: expected InvalidParameterValueException, got {body}"
    );
    assert!(
        body["instanceRecommendations"].is_null(),
        "case {case:?}: a refused read reports no recommendations, got {body}"
    );
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The `instanceArn` of every recommendation in a response, in the order reported.
fn reported_arns(body: &Value) -> Vec<String> {
    body["instanceRecommendations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| row["instanceArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The identifier of every entry of a response's `errors` list, in the order reported.
fn reported_error_identifiers(body: &Value) -> Vec<String> {
    body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|error| error["identifier"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The recommendation a response carries for one analysed instance.
fn recommendation_for(body: &Value, instance_id: &str) -> Value {
    let wanted = instance_arn(instance_id);
    body["instanceRecommendations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {instance_id}: instanceRecommendations must be a list: {body}")
        })
        .iter()
        .find(|row| row["instanceArn"] == json!(wanted))
        .cloned()
        .unwrap_or_else(|| panic!("case {instance_id}: no recommendation for {wanted}: {body}"))
}

/// A number the wire carries, or `0` for one the protocol omitted because it equals the
/// member's modelled default.
fn number(value: &Value, member: &str) -> f64 {
    value[member].as_f64().unwrap_or(0.0)
}

/// The recommendation options of a recommendation, in the order reported.
fn options(recommendation: &Value) -> Vec<Value> {
    recommendation["recommendationOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One reported metric list as (name, statistic, value) triples, so a comparison is on the
/// figures rather than on how the protocol rendered them.
fn reported_metrics(value: &Value, member: &str) -> Vec<(String, String, f64)> {
    value[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap_or_default().to_string(),
                entry["statistic"].as_str().unwrap_or_default().to_string(),
                number(entry, "value"),
            )
        })
        .collect()
}

/// The estimated monthly on-demand cost of one instance of `configuration`: the price of one
/// instance-hour × the hours in a month, which is how this service prices an instance.
fn monthly_cost(configuration: &InstanceConfiguration) -> f64 {
    round_hundredths(configuration.hourly_price * HOURS_PER_MONTH)
}

/// Two decimal places, the precision every money figure and percentage of this service is
/// reported to.
fn round_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Every analysed instance of the caller's account is reported with the members the stored
/// analysis supports, and every option is a candidate the analysis found — ranked, with the
/// utilization projected onto it and the saving adopting it offers. Reading it twice changes
/// nothing a later caller sees.
#[tokio::test]
async fn every_analysed_instance_is_reported_from_its_stored_analysis() {
    let (router, engine) = emulator_with_corpus().await;
    let before = snapshot(&engine);
    let body = recommendations(&router, "the whole analysis", json!({})).await;

    let mut reported = reported_arns(&body);
    reported.sort();
    let mut expected: Vec<String> = corpus()
        .iter()
        .map(|instance| instance_arn(&instance.instance_id))
        .collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "one recommendation per analysed instance of this account, got {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "a request naming no instance reports no per-identifier error, got {body}"
    );

    // (case, the finding the analysis reaches, the candidates it found)
    let rows: [(&str, &str, Vec<InstanceConfiguration>); 4] = [
        // Sizing down means at least one specification can shrink: over-provisioned.
        (SIZED_DOWN, FINDING_OVERPROVISIONED, vec![m5_xlarge()]),
        // Sizing up means a specification did not meet the workload: under-provisioned.
        (SIZED_UP, FINDING_UNDERPROVISIONED, vec![m5_xlarge()]),
        // Nothing to move to: optimized, and no option to report.
        (ALREADY_RIGHT_SIZED, FINDING_OPTIMIZED, Vec::new()),
        (
            TWO_ARCHITECTURES,
            FINDING_OVERPROVISIONED,
            vec![m5_xlarge(), m6g_xlarge()],
        ),
    ];

    for (instance_id, finding, candidates) in rows {
        let analysed = analysed_instance(instance_id);
        let recommendation = recommendation_for(&body, instance_id);
        let case = instance_id;

        assert_eq!(
            recommendation["accountId"],
            json!(ACCOUNT),
            "case {case:?}: every recommendation is of the caller's own account, got \
             {recommendation}"
        );
        assert_eq!(
            recommendation["currentInstanceType"],
            json!(analysed.current.instance_type),
            "case {case:?}: expected the type the instance runs, got {recommendation}"
        );
        assert_eq!(
            recommendation["finding"],
            json!(finding),
            "case {case:?}: expected finding {finding}, got {recommendation}"
        );
        assert_eq!(
            recommendation["findingReasonCodes"],
            json!(analysed.finding_reason_codes),
            "case {case:?}: expected the stored reason codes, got {recommendation}"
        );
        assert_eq!(
            recommendation["currentPerformanceRisk"],
            json!(analysed.current_performance_risk),
            "case {case:?}: expected the stored performance risk, got {recommendation}"
        );
        assert_eq!(
            recommendation["instanceState"],
            json!(INSTANCE_STATE_RUNNING),
            "case {case:?}: expected the state the instance was analysed in, got \
             {recommendation}"
        );
        assert_eq!(
            recommendation["inferredWorkloadTypes"],
            json!(analysed.inferred_workload_types),
            "case {case:?}: expected the inferred workload types, got {recommendation}"
        );
        // "The number of days for which utilization metrics were analyzed": the length of the
        // look-back period in force, which no preference has changed here (`DAYS_14`).
        assert_eq!(
            number(&recommendation, "lookBackPeriodInDays"),
            14.0,
            "case {case:?}: expected the default look-back period's length, got {recommendation}"
        );
        assert_eq!(
            recommendation["idle"],
            json!("False"),
            "case {case:?}: this service holds no idle recommendation for the instance, got \
             {recommendation}"
        );
        assert_eq!(
            recommendation["recommendationSources"],
            json!([{
                "recommendationSourceArn": instance_arn(instance_id),
                "recommendationSourceType": "Ec2Instance",
            }]),
            "case {case:?}: the source of an instance recommendation is that instance, got \
             {recommendation}"
        );
        // An account that prefers no external metrics provider has no integration at all.
        assert_eq!(
            recommendation["externalMetricStatus"]["statusCode"],
            json!("NO_EXTERNAL_METRIC_SET"),
            "case {case:?}: expected no external metrics integration, got {recommendation}"
        );
        // The `Name` tag is what names an instance; one that carries no tag carries no name.
        let name = analysed
            .tags
            .iter()
            .find(|(key, _)| key == NAME_TAG_KEY)
            .map(|(_, value)| value.clone());
        assert_eq!(
            recommendation["instanceName"],
            name.map_or(Value::Null, Value::from),
            "case {case:?}: expected the name the Name tag gives, got {recommendation}"
        );
        let tags: Vec<Value> = analysed
            .tags
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect();
        assert_eq!(
            recommendation["tags"],
            if tags.is_empty() {
                Value::Null
            } else {
                Value::from(tags)
            },
            "case {case:?}: expected the stored tags, got {recommendation}"
        );

        // Only the observations recorded under `Maximum` are reported.
        let expected_metrics: Vec<(String, String, f64)> = analysed
            .utilization
            .iter()
            .filter(|metric| metric.statistic == MAXIMUM)
            .map(|metric| (metric.name.clone(), MAXIMUM.to_string(), metric.value))
            .collect();
        assert_eq!(
            reported_metrics(&recommendation, "utilizationMetrics"),
            expected_metrics,
            "case {case:?}: expected the observed Maximum utilization, got {recommendation}"
        );

        let reported_options = options(&recommendation);
        assert_eq!(
            reported_options.len(),
            candidates.len(),
            "case {case:?}: expected one option per candidate, got {recommendation}"
        );
        for (index, candidate) in candidates.iter().enumerate() {
            let option = &reported_options[index];
            let case = format!("{case}/{}", candidate.instance_type);
            assert_eq!(
                option["instanceType"],
                json!(candidate.instance_type),
                "case {case:?}: expected the candidate's instance type, got {option}"
            );
            // "The top recommendation option is ranked as 1."
            let rank = (index + 1) as f64;
            assert_eq!(
                number(option, "rank"),
                rank,
                "case {case:?}: expected rank {rank}, got {option}"
            );
            // The saving is what the move takes off the monthly bill, and the percentage is
            // that saving as a share of what the instance costs today; a move that costs no
            // less saves nothing rather than a negative amount.
            let current = monthly_cost(&analysed.current);
            let saved = round_hundredths((current - monthly_cost(candidate)).max(0.0));
            let percentage = round_hundredths(saved / current * 100.0);
            assert_eq!(
                number(&option["savingsOpportunity"], "savingsOpportunityPercentage"),
                percentage,
                "case {case:?}: expected a saving of {percentage}% of {current}, got {option}"
            );
            let savings = &option["savingsOpportunity"]["estimatedMonthlySavings"];
            assert_eq!(
                savings["currency"],
                json!("USD"),
                "case {case:?}: expected the saving in USD, got {option}"
            );
            assert_eq!(
                number(savings, "value"),
                saved,
                "case {case:?}: expected a monthly saving of {saved}, got {option}"
            );
            assert!(
                option["savingsOpportunityAfterDiscounts"].is_null(),
                "case {case:?}: the after-discount member is reported only once the account \
                 activates the preference, got {option}"
            );
            // Only `Cpu` and `Memory` are projected, each scaled by the ratio of the capacity
            // it was observed on to the capacity it is projected onto — and no percentage can
            // pass 100.
            let expected_projected: Vec<(String, String, f64)> = analysed
                .utilization
                .iter()
                .filter(|metric| metric.statistic == MAXIMUM)
                .filter(|metric| metric.name == "Cpu" || metric.name == "Memory")
                .map(|metric| {
                    let (from, to) = if metric.name == "Memory" {
                        (analysed.current.memory_gib, candidate.memory_gib)
                    } else {
                        (analysed.current.vcpus, candidate.vcpus)
                    };
                    let projected = round_hundredths((metric.value * from / to).clamp(0.0, 100.0));
                    (metric.name.clone(), MAXIMUM.to_string(), projected)
                })
                .collect();
            assert_eq!(
                reported_metrics(option, "projectedUtilizationMetrics"),
                expected_projected,
                "case {case:?}: expected the projected Cpu and Memory utilization, got {option}"
            );
            // A candidate of another CPU architecture is a platform difference, and moving to
            // it is more effort than staying on the same architecture.
            let differs = candidate.architecture != analysed.current.architecture;
            assert_eq!(
                option["platformDifferences"],
                if differs {
                    json!(["Architecture"])
                } else {
                    json!([])
                },
                "case {case:?}: expected the architecture difference to be stated, got {option}"
            );
            let effort = if !differs {
                "VeryLow"
            } else if analysed.inferred_workload_types.is_empty() {
                "Medium"
            } else {
                "Low"
            };
            assert_eq!(
                option["migrationEffort"],
                json!(effort),
                "case {case:?}: expected migration effort {effort}, got {option}"
            );
            // The risk an option runs is bounded by the model's own 0..4 scale.
            let risk = number(option, "performanceRisk");
            assert!(
                (0.0..=4.0).contains(&risk),
                "case {case:?}: expected a performance risk in 0..4, got {risk}"
            );
        }
    }

    // A read is a READ: the same call answers the same thing, and the store did not move.
    let again = recommendations(&router, "the same read again", json!({})).await;
    assert_eq!(
        again, body,
        "two identical reads must answer identically, got {again}"
    );
    assert_eq!(
        snapshot(&engine),
        before,
        "reading recommendations must not write to the store"
    );
}

/// What a request SELECTS: `accountIds` reads the caller's own account only, `instanceArns`
/// picks instances out of the analysis, an identifier the analysis holds no recommendation
/// for is simply absent from the answer, and an ARN of another Region names an instance this
/// endpoint does not analyse.
#[tokio::test]
async fn account_ids_and_instance_arns_select_what_is_reported() {
    let (router, _engine) = emulator_with_corpus().await;
    let all = reported_arns(&recommendations(&router, "the whole analysis", json!({})).await);
    let unknown = instance_arn("i-0c1b2c3d4e5f8ffff");
    let other_region = instance_arn_in("eu-west-1", SIZED_UP);
    // The SAME instance of the SAME Region, named in another partition: an ARN form the
    // service parses, which names an instance this endpoint analyses.
    let other_partition = format!("arn:aws-cn:ec2:{REGION}:{ACCOUNT}:instance/{SIZED_UP}");

    // (case, request, the ARNs reported)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the caller's own account, named",
            json!({"accountIds": [ACCOUNT]}),
            all.clone(),
        ),
        ("an empty account list", json!({"accountIds": []}), all.clone()),
        (
            "one instance",
            json!({"instanceArns": [instance_arn(SIZED_UP)]}),
            vec![instance_arn(SIZED_UP)],
        ),
        (
            "two instances",
            json!({"instanceArns": [instance_arn(SIZED_DOWN), instance_arn(TWO_ARCHITECTURES)]}),
            vec![instance_arn(SIZED_DOWN), instance_arn(TWO_ARCHITECTURES)],
        ),
        (
            "the same instance twice",
            json!({"instanceArns": [instance_arn(SIZED_UP), instance_arn(SIZED_UP)]}),
            vec![instance_arn(SIZED_UP)],
        ),
        (
            "an instance this analysis does not hold",
            json!({"instanceArns": [unknown.clone()]}),
            Vec::new(),
        ),
        (
            "the same unheld instance twice",
            json!({"instanceArns": [unknown.clone(), unknown.clone()]}),
            Vec::new(),
        ),
        (
            "one held and one unheld instance",
            json!({"instanceArns": [instance_arn(SIZED_UP), unknown.clone()]}),
            vec![instance_arn(SIZED_UP)],
        ),
        (
            "an instance of another Region",
            json!({"instanceArns": [other_region.clone()]}),
            Vec::new(),
        ),
        (
            // The identifier names an analysed instance of this Region, so it is answered
            // with that instance's recommendation — whatever partition the ARN was written
            // in.
            "an instance of this Region named in another partition",
            json!({"instanceArns": [other_partition.clone()]}),
            vec![instance_arn(SIZED_UP)],
        ),
    ];

    for (case, request, expected) in rows {
        let body = recommendations(&router, case, request).await;
        let mut reported = reported_arns(&body);
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected exactly {wanted:?}, got {body}"
        );
        // An identifier with no recommendation is absent from the answer and is NOT a
        // per-resource failure of the request: `errors` carries the instances an analysis
        // rejected as unsupported, which this inventory holds none of.
        assert_eq!(
            reported_error_identifiers(&body),
            Vec::<String>::new(),
            "case {case:?}: expected no errors entry, got {body}"
        );
    }

    // The endpoint's Region is what the reported ARNs name: the same analysis read at another
    // Region's endpoint reports that Region's ARNs, which is why an ARN of another Region
    // selects nothing here.
    let (status, body) = call_in_region(
        &router,
        "eu-west-1",
        "GetEC2InstanceRecommendations",
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "another Region's endpoint: got {body}");
    let mut reported = reported_arns(&body);
    reported.sort();
    let mut wanted: Vec<String> = corpus()
        .iter()
        .map(|instance| instance_arn_in("eu-west-1", &instance.instance_id))
        .collect();
    wanted.sort();
    assert_eq!(
        reported, wanted,
        "another Region's endpoint reports its own Region's ARNs, got {body}"
    );
}

/// The `Filters` of this surface select on the ANALYSIS: a filter matches on the member it
/// names, several values are ORed, several filters are ANDed, and a filter carrying no values
/// constrains nothing.
#[tokio::test]
async fn filters_select_by_the_analysis_they_name() {
    let (router, _engine) = emulator_with_corpus().await;
    let all = reported_arns(&recommendations(&router, "the whole analysis", json!({})).await);

    // (case, filters, the instances reported)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "one Finding value",
            json!([{"name": "Finding", "values": ["Underprovisioned"]}]),
            vec![instance_arn(SIZED_UP)],
        ),
        (
            "two Finding values",
            json!([{"name": "Finding", "values": ["Underprovisioned", "Optimized"]}]),
            vec![instance_arn(SIZED_UP), instance_arn(ALREADY_RIGHT_SIZED)],
        ),
        (
            "a Finding nothing in the analysis reaches",
            json!([{"name": "Finding", "values": ["NotOptimized"]}]),
            Vec::new(),
        ),
        (
            "one FindingReasonCodes value",
            json!([{"name": "FindingReasonCodes", "values": ["MemoryOverprovisioned"]}]),
            vec![instance_arn(SIZED_DOWN)],
        ),
        (
            "an InferredWorkloadTypes value",
            json!([{"name": "InferredWorkloadTypes", "values": ["AmazonEmr"]}]),
            vec![instance_arn(TWO_ARCHITECTURES)],
        ),
        (
            "the RecommendationSourceType this reader reports",
            json!([{"name": "RecommendationSourceType", "values": ["Ec2Instance"]}]),
            all.clone(),
        ),
        (
            "another RecommendationSourceType",
            json!([{"name": "RecommendationSourceType", "values": ["EbsVolume"]}]),
            Vec::new(),
        ),
        (
            "two filters, ANDed",
            json!([
                {"name": "Finding", "values": ["Overprovisioned"]},
                {"name": "FindingReasonCodes", "values": ["CPUOverprovisioned"]},
            ]),
            vec![instance_arn(TWO_ARCHITECTURES)],
        ),
        (
            "two filters that select nothing together",
            json!([
                {"name": "Finding", "values": ["Optimized"]},
                {"name": "FindingReasonCodes", "values": ["CPUOverprovisioned"]},
            ]),
            Vec::new(),
        ),
        (
            "a filter carrying no values",
            json!([{"name": "Finding", "values": []}]),
            all.clone(),
        ),
    ];

    for (case, filters, expected) in rows {
        let body = recommendations(&router, case, json!({"filters": filters})).await;
        let mut reported = reported_arns(&body);
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected exactly {wanted:?}, got {body}"
        );
    }
}

/// One row of [`a_cpu_vendor_architecture_preference_selects_the_options`]: the CPU vendor
/// architecture preference a request states, the option instance types it then reports for
/// the two-architecture instance, and the architectures those options consist of.
struct ArchitectureCase {
    case: &'static str,
    preference: Option<Value>,
    instance_types: Vec<&'static str>,
    architectures: Vec<&'static str>,
}

/// `cpuVendorArchitectures` selects which candidate instance types the options consist of:
/// "when you specify AWS_ARM64 … Compute Optimizer returns recommendations that consist of
/// Graviton instance types only", and the preferences a recommendation says it was generated
/// under name exactly the architectures its options carry.
#[tokio::test]
async fn a_cpu_vendor_architecture_preference_selects_the_options() {
    let (router, _engine) = emulator_with_corpus().await;

    let rows = vec![
        ArchitectureCase {
            case: "no preference",
            preference: None,
            instance_types: vec!["m5.xlarge", "m6g.xlarge"],
            architectures: vec![CPU_ARCHITECTURE_CURRENT, CPU_ARCHITECTURE_AWS_ARM64],
        },
        ArchitectureCase {
            case: "Graviton only",
            preference: Some(json!({"cpuVendorArchitectures": ["AWS_ARM64"]})),
            instance_types: vec!["m6g.xlarge"],
            architectures: vec![CPU_ARCHITECTURE_AWS_ARM64],
        },
        ArchitectureCase {
            case: "the current architecture only",
            preference: Some(json!({"cpuVendorArchitectures": ["CURRENT"]})),
            instance_types: vec!["m5.xlarge"],
            architectures: vec![CPU_ARCHITECTURE_CURRENT],
        },
        ArchitectureCase {
            case: "both",
            preference: Some(json!({"cpuVendorArchitectures": ["CURRENT", "AWS_ARM64"]})),
            instance_types: vec!["m5.xlarge", "m6g.xlarge"],
            architectures: vec![CPU_ARCHITECTURE_CURRENT, CPU_ARCHITECTURE_AWS_ARM64],
        },
    ];

    for row in rows {
        let ArchitectureCase {
            case,
            preference,
            instance_types: expected,
            architectures,
        } = row;
        let mut request = json!({"instanceArns": [instance_arn(TWO_ARCHITECTURES)]});
        if let Some(preference) = preference {
            request["recommendationPreferences"] = preference;
        }
        let body = recommendations(&router, case, request).await;
        let recommendation = recommendation_for(&body, TWO_ARCHITECTURES);
        let reported: Vec<String> = options(&recommendation)
            .iter()
            .map(|option| {
                option["instanceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected options {expected:?}, got {recommendation}"
        );
        // Every option is ranked from 1 among the options REPORTED.
        let ranks: Vec<f64> = options(&recommendation)
            .iter()
            .map(|option| number(option, "rank"))
            .collect();
        let wanted: Vec<f64> = (1..=expected.len()).map(|rank| rank as f64).collect();
        assert_eq!(
            ranks, wanted,
            "case {case:?}: expected ranks {wanted:?}, got {recommendation}"
        );
        // "Describes the CPU vendor and architecture for an instance … recommendation": the
        // architectures the reported options actually consist of.
        assert_eq!(
            recommendation["effectiveRecommendationPreferences"]["cpuVendorArchitectures"],
            json!(architectures),
            "case {case:?}: expected the architectures the options consist of, got \
             {recommendation}"
        );
    }
}

/// `maxResults` caps a page and the `nextToken` resumes exactly where the previous page
/// stopped: whatever page size walks the analysis, the sequence it reports is the same one an
/// unpaged read reports, each instance exactly once.
#[tokio::test]
async fn pages_walk_every_recommendation_exactly_once() {
    let (router, _engine) = emulator_with_corpus().await;
    let all = reported_arns(&recommendations(&router, "the unpaged read", json!({})).await);
    assert_eq!(
        all.len(),
        corpus().len(),
        "the whole analysis: expected {} rows, got {all:?}",
        corpus().len()
    );

    // Rows: every page size from one up to one past the sequence, plus `0` — the modelled
    // range's lower bound, which carries no documented meaning and so asks for no size at
    // all, which is the service's own default page.
    for page_size in [0, 1, 2, 3, 4, 5] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut finished = false;
        // One page per row plus one, so a token that never terminates fails the test instead
        // of hanging it.
        for page in 0..=corpus().len() {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let body = recommendations(&router, &case, request).await;
            let reported = reported_arns(&body);
            let remaining = all.len() - walked.len();
            let expected_len = if page_size == 0 {
                remaining.min(DEFAULT_PAGE_SIZE)
            } else {
                page_size.min(remaining)
            };
            assert_eq!(
                reported.len(),
                expected_len,
                "case {case:?}: page {page} must carry {expected_len} rows, got {body}"
            );
            walked.extend(reported);
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => {
                    finished = true;
                    break;
                }
            }
        }
        assert!(
            finished,
            "case {case:?}: the walk did not finish within {} pages: {walked:?}",
            corpus().len() + 1
        );
        assert_eq!(
            walked, all,
            "case {case:?}: a paged walk reports the unpaged sequence, got {walked:?}"
        );
    }
}

/// An input the service cannot honour is REFUSED, never answered with a selection of
/// something else: an account it cannot report on, an ARN that is not an instance's, a filter
/// outside this surface's vocabulary, and a page token it did not issue.
#[tokio::test]
async fn an_unhonourable_request_is_refused() {
    let (router, engine) = emulator_with_corpus().await;
    let before = snapshot(&engine);
    let cross_account = "Invalid permission to access another account. If this is an \
                         organization account, request the management account to enable \
                         service access.";

    // (case, request, the message the refusal carries)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "another account's id",
            json!({"accountIds": ["210987654321"]}),
            cross_account,
        ),
        (
            "an instance ARN of another account",
            json!({"instanceArns": [
                "arn:aws:ec2:us-east-1:210987654321:instance/i-0123456789abcdef0"
            ]}),
            cross_account,
        ),
        (
            "an ARN that is not an instance's",
            json!({"instanceArns": [
                format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-0123456789abcdef0")
            ]}),
            "Invalid instance arn.",
        ),
        (
            "a value that is not an ARN at all",
            json!({"instanceArns": ["i-0123456789abcdef0"]}),
            "Invalid instance arn.",
        ),
        (
            "a page token this service did not issue",
            json!({"nextToken": "not-a-token"}),
            "The value supplied for parameter nextToken is not valid.",
        ),
    ];

    for (case, request, message) in rows {
        let reported = refused(&router, case, request).await;
        assert_eq!(
            reported, message,
            "case {case:?}: expected the refusal {message:?}, got {reported:?}"
        );
    }

    // No request may name more instances than this service answers about in one call: the
    // bound is the crate's ([`MAX_REQUESTED_IDENTIFIERS`]), so a list at it is answered and the
    // first list past it is refused, naming the member and the bound.
    let at_the_bound: Vec<String> = (0..MAX_REQUESTED_IDENTIFIERS)
        .map(|index| instance_arn(&format!("i-0c1b2c3d4e5f9{index:05}")))
        .collect();
    let body = recommendations(
        &router,
        "as many instances as the bound allows",
        json!({"instanceArns": at_the_bound}),
    )
    .await;
    assert_eq!(
        reported_error_identifiers(&body),
        Vec::<String>::new(),
        "a list at the bound is answered, and an identifier with no recommendation is no \
         error of the request: got {body}"
    );
    let past_the_bound: Vec<String> = (0..=MAX_REQUESTED_IDENTIFIERS)
        .map(|index| instance_arn(&format!("i-0c1b2c3d4e5f9{index:05}")))
        .collect();
    let message = refused(
        &router,
        "one instance more than the bound allows",
        json!({"instanceArns": past_the_bound}),
    )
    .await;
    assert!(
        message.contains("instanceArns")
            && message.contains(&MAX_REQUESTED_IDENTIFIERS.to_string()),
        "the refusal must name the member and the bound, got {message:?}"
    );

    // A filter value outside the vocabulary the name selects on, and a filter with no name at
    // all: refused naming the member, rather than silently matching nothing.
    for (case, filters) in [
        (
            "a value outside the Finding vocabulary",
            json!([{"name": "Finding", "values": ["Enormous"]}]),
        ),
        ("a filter with no name", json!([{"values": ["Optimized"]}])),
    ] {
        let message = refused(&router, case, json!({"filters": filters})).await;
        assert!(
            message.contains("filters"),
            "case {case:?}: the refusal must name the member, got {message:?}"
        );
    }

    assert_eq!(
        snapshot(&engine),
        before,
        "a refused read must not write to the store"
    );
}

/// The enrollment gate: Compute Optimizer analyses an account's resources only once it has
/// opted in, and only once the analysis that enrollment starts is due — so an account that
/// never opted in is refused with the modelled 403, and one that has just opted in reads the
/// empty sequence rather than an analysis nothing has produced yet.
#[tokio::test]
async fn the_analysis_is_readable_only_once_the_enrollment_makes_it_due() {
    let (router, engine) = emulator_with_engine();
    record(&engine, corpus());

    let (status, body) = call(&router, "GetEC2InstanceRecommendations", json!({})).await;
    assert_eq!(
        status, 403,
        "an account that never opted in: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "an account that never opted in: expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;
    let body = recommendations(&router, "just opted in", json!({})).await;
    assert_eq!(
        body["instanceRecommendations"],
        json!([]),
        "the analysis an enrollment starts is not due yet, got {body}"
    );

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let body = recommendations(&router, "the analysis is due", json!({})).await;
    assert_eq!(
        reported_arns(&body).len(),
        corpus().len(),
        "once the analysis is due every analysed instance is reported, got {body}"
    );
}

/// The preferences a recommendation says it was generated under are the account's STORED
/// ones: a longer look-back period is reported as the days analysed, and activating the
/// savings-estimation-mode preference is what makes an option carry its after-discount
/// savings.
#[tokio::test]
async fn the_reported_preferences_are_the_account_s_stored_preferences() {
    let (router, _engine) = emulator_with_corpus().await;

    // (case, the preference written, the days reported, whether an option carries the
    // after-discount member)
    let rows: Vec<(&str, Value, f64, bool)> = vec![
        (
            "a longer look-back period",
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "AccountId", "value": ACCOUNT},
                "lookBackPeriod": "DAYS_93",
            }),
            93.0,
            false,
        ),
        (
            "savings net of discounts",
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "AccountId", "value": ACCOUNT},
                "lookBackPeriod": "DAYS_93",
                "savingsEstimationMode": "AfterDiscounts",
            }),
            93.0,
            true,
        ),
    ];

    for (case, preference, days, after_discounts) in rows {
        let (status, body) =
            call(&router, "PutRecommendationPreferences", preference.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing {preference} expected 200, got {status}: {body}"
        );
        let body = recommendations(
            &router,
            case,
            json!({"instanceArns": [instance_arn(SIZED_DOWN)]}),
        )
        .await;
        let recommendation = recommendation_for(&body, SIZED_DOWN);
        assert_eq!(
            number(&recommendation, "lookBackPeriodInDays"),
            days,
            "case {case:?}: expected {days} days analysed, got {recommendation}"
        );
        assert_eq!(
            recommendation["effectiveRecommendationPreferences"]["lookBackPeriod"],
            json!("DAYS_93"),
            "case {case:?}: expected the stored look-back preference, got {recommendation}"
        );
        let option = options(&recommendation)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected an option: {recommendation}"));
        assert_eq!(
            !option["savingsOpportunityAfterDiscounts"].is_null(),
            after_discounts,
            "case {case:?}: expected the after-discount member reported={after_discounts}, got \
             {option}"
        );
        if after_discounts {
            // This emulator records no Savings Plans or Reserved Instances commitment against
            // the account's spend, so the saving after discounts is the saving before them.
            assert_eq!(
                number(
                    &option["savingsOpportunityAfterDiscounts"]["estimatedMonthlySavings"],
                    "value"
                ),
                number(
                    &option["savingsOpportunity"]["estimatedMonthlySavings"],
                    "value"
                ),
                "case {case:?}: expected the same saving before and after discounts, got {option}"
            );
        }
    }
}
