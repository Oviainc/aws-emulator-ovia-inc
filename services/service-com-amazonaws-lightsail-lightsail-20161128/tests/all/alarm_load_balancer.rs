// SPDX-License-Identifier: CC0-1.0

//! Alarms on a LOAD BALANCER, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `PutAlarm` resolves `monitoredResourceName` against
//! every resource type the service says an alarm can monitor — "Instances, load
//! balancers, and relational databases are the only Lightsail resources that can
//! currently be monitored by alarms"
//! (`smithy_query(query="doc",shape="PutAlarmRequest",member="monitoredResourceName")`)
//! — each addressed the way its own operations address it, so a load balancer is
//! the one of that name in the Region the request is made in. The alarm that
//! comes of it is an alarm like any other: it reports the watched load
//! balancer's name, ARN and `LoadBalancer` resource type, lives in that load
//! balancer's Region, carries the published `unit` and `statistic` of the load
//! balancer metric it watches, and is read by `GetAlarms`, moved by `TestAlarm`
//! and removed by `DeleteAlarm`. A metric a load balancer does not publish, a
//! third alarm on one of its metrics, or a name no load balancer of the
//! request's Region answers to is refused and stores nothing.
//!
//! The harness, the alarm reads and the load balancer stand-up are the ones this
//! test target already exposes.

use serde_json::{json, Value};

use crate::alarm_configuration::{alarm, alarms, names};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
use crate::load_balancer_instance_attachment::{
    create_load_balancer, load_balancer, wait_for_load_balancer_state,
};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN
/// carries.
const ACCOUNT: &str = "000000000000";

/// `Alarm$period`: the five minutes one evaluation period covers, in seconds
/// ("Each evaluation period is five minutes long",
/// `PutAlarmRequest$evaluationPeriods`).
const ALARM_PERIOD_SECONDS: i64 = 300;

/// The state a new alarm is in ("When this action creates an alarm, the alarm
/// state is immediately set to INSUFFICIENT_DATA", `PutAlarm`).
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The modelled `ResourceType` value a load balancer reports.
const LOAD_BALANCER: &str = "LoadBalancer";

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The `LoadBalancerState` a load balancer settles in.
const ACTIVE: &str = "active";

/// The most alarms one metric of one monitored resource accepts ("You can
/// configure up to two alarms per metric",
/// `smithy_query(query="doc",shape="PutAlarmRequest",member="metricName")`).
const MAX_ALARMS_PER_METRIC: usize = 2;

// ---------------------------------------------------------------------------
// The load balancer rows of the published alarm catalog
// ---------------------------------------------------------------------------

/// One load balancer metric an alarm can watch, with the `unit` it is published
/// in and the `statistic` an alarm on it reports — neither of which `PutAlarm`
/// takes, so both are properties of the metric.
struct Metric {
    name: &'static str,
    unit: &'static str,
    statistic: &'static str,
}

/// Every metric an alarm on a load balancer can watch.
///
/// Written out here independently of the service's own catalog, so these rows
/// pin the published list rather than restating whatever the code holds.
///
/// SOURCE: WHICH metrics a load balancer publishes to an alarm is the "Load
/// balancers" entry of
/// `smithy_query(query="doc",shape="PutAlarmRequest",member="metricName")`; the
/// `unit` of each is the "published unit" and the `statistic` the most useful
/// one stated by
/// `smithy_query(query="doc",shape="GetLoadBalancerMetricDataRequest",member="metricName")`
/// — `Average` where it names several including `Average`, otherwise the one it
/// names.
const METRICS: &[Metric] = &[
    Metric {
        name: "ClientTLSNegotiationErrorCount",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "HealthyHostCount",
        unit: "Count",
        statistic: "Average",
    },
    Metric {
        name: "UnhealthyHostCount",
        unit: "Count",
        statistic: "Average",
    },
    Metric {
        name: "HTTPCode_LB_4XX_Count",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "HTTPCode_LB_5XX_Count",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "HTTPCode_Instance_2XX_Count",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "HTTPCode_Instance_3XX_Count",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "HTTPCode_Instance_4XX_Count",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "HTTPCode_Instance_5XX_Count",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "InstanceResponseTime",
        unit: "Seconds",
        statistic: "Average",
    },
    Metric {
        name: "RejectedConnectionCount",
        unit: "Count",
        statistic: "Sum",
    },
    Metric {
        name: "RequestCount",
        unit: "Count",
        statistic: "Sum",
    },
];

/// The Regions the cases spread over: a load balancer is a REGIONAL resource, so
/// the Region a request is made in is part of what addresses one. Each holds a
/// load balancer of the SAME name, so which one an alarm watches is decided by
/// the Region the put is made in and nothing else.
const REGIONS: &[&str] = &["us-east-1", "eu-central-1", "ap-southeast-2"];

/// The load balancer name every Region in [`REGIONS`] holds.
const WATCHED: &str = "watched-lb";

/// The comparison operators the rows cycle through: every member of the modelled
/// `ComparisonOperator` enum (`smithy_query(query="shape",shape="ComparisonOperator")`).
const COMPARISON_OPERATORS: &[&str] = &[
    "GreaterThanOrEqualToThreshold",
    "GreaterThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanThreshold",
];

/// The evaluation windows the rows cycle through: both published bounds of the
/// documented range ("You can specify a minimum valuation period of 1 (5
/// minutes), and a maximum evaluation period of 288 (24 hours)",
/// `PutAlarmRequest$evaluationPeriods`) and two values between them.
const EVALUATION_PERIODS: &[i64] = &[1, 4, 36, 288];

/// Create the load balancer [`WATCHED`] in every Region of [`REGIONS`], each on
/// its own instance port, answering the ARN each Region's own
/// `GetLoadBalancer` reports for it.
///
/// The load balancer of the first Region is waited into `active` and the others
/// are left as the create answered, so the rows cover a settled load balancer
/// and ones still provisioning: an alarm watches a resource whatever lifecycle
/// state that resource is in.
async fn watched_load_balancers(emulator: &Emulator) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, region) in REGIONS.iter().enumerate() {
        create_load_balancer(emulator, region, WATCHED, 80 + index as i64).await;
        if index == 0 {
            wait_for_load_balancer_state(emulator, region, WATCHED, ACTIVE).await;
        }
        let balancer = load_balancer(emulator, region, WATCHED).await;
        assert_eq!(
            balancer["location"]["regionName"].as_str(),
            Some(*region),
            "case load balancer stand-up in {region}: expected it in that Region, got {balancer:?}"
        );
        arns.push(
            balancer["arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case load balancer stand-up in {region}: expected an arn, got {balancer:?}")
                })
                .to_string(),
        );
    }
    arns
}

/// The `PutAlarm` body watching [`WATCHED`] on `metric`.
fn put_body(
    alarm_name: &str,
    metric: &str,
    comparison_operator: &str,
    threshold: f64,
    evaluation_periods: i64,
) -> Value {
    json!({
        "alarmName": alarm_name,
        "monitoredResourceName": WATCHED,
        "metricName": metric,
        "comparisonOperator": comparison_operator,
        "threshold": threshold,
        "evaluationPeriods": evaluation_periods,
    })
}

// ---------------------------------------------------------------------------
// Put and read back
// ---------------------------------------------------------------------------

/// RULE: a load balancer this account holds in the Region the request is made in
/// is a resource an alarm can monitor, so every metric a load balancer publishes
/// to an alarm is stored against THAT load balancer — its name, its ARN and the
/// `LoadBalancer` resource type — in its Region, with the published unit and
/// statistic of the metric.
#[tokio::test]
async fn every_load_balancer_metric_is_watched_against_the_load_balancer_of_the_requests_region() {
    let emulator = Emulator::new();
    let arns = watched_load_balancers(&emulator).await;

    for (index, metric) in METRICS.iter().enumerate() {
        // The rows differ along everything the rule generalises over: the
        // metric, the Region the put is made in (and so which load balancer of
        // that name it watches), the comparison operator, the threshold and the
        // evaluation window.
        let region = REGIONS[index % REGIONS.len()];
        let expected_arn = &arns[index % REGIONS.len()];
        let comparison_operator = COMPARISON_OPERATORS[index % COMPARISON_OPERATORS.len()];
        let evaluation_periods = EVALUATION_PERIODS[index % EVALUATION_PERIODS.len()];
        let threshold = 0.5 + index as f64;
        let alarm_name = format!("lb-{}", metric.name);
        let case = format!("{} in {region}", metric.name);

        let put = emulator
            .ok_at(
                None,
                Some(region),
                "PutAlarm",
                put_body(
                    &alarm_name,
                    metric.name,
                    comparison_operator,
                    threshold,
                    evaluation_periods,
                ),
            )
            .await;
        let operations = put["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {put:?}"));
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {put:?}"
        );
        assert_eq!(
            operations[0]["location"]["regionName"].as_str(),
            Some(region),
            "case {case}: expected the operation in the load balancer's Region, got {:?}",
            operations[0]
        );

        let stored = alarm(&emulator, &alarm_name, &case).await;

        // The monitored resource is the load balancer of the put's Region.
        assert_eq!(
            stored["monitoredResourceInfo"]["name"].as_str(),
            Some(WATCHED),
            "case {case}: expected the watched load balancer's name, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["resourceType"].as_str(),
            Some(LOAD_BALANCER),
            "case {case}: expected the {LOAD_BALANCER} resource type, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: expected the arn GetLoadBalancer reports in {region}, got {stored:?}"
        );
        assert_eq!(
            stored["location"]["regionName"].as_str(),
            Some(region),
            "case {case}: expected the alarm in the load balancer's Region, got {stored:?}"
        );
        assert!(
            stored["location"]["availabilityZone"].is_null(),
            "case {case}: an alarm is not zonal, got {stored:?}"
        );
        let prefix = format!("arn:aws:lightsail:{region}:{ACCOUNT}:Alarm/");
        let arn = stored["arn"].as_str().unwrap_or_default();
        assert!(
            arn.starts_with(&prefix) && is_uuid_shaped(&arn[prefix.len().min(arn.len())..]),
            "case {case}: expected an arn under {prefix} with a uuid-shaped id, got {arn:?}"
        );

        // What the metric, not the request, decides.
        assert_eq!(
            stored["unit"].as_str(),
            Some(metric.unit),
            "case {case}: expected the published unit {}, got {stored:?}",
            metric.unit
        );
        assert_eq!(
            stored["statistic"].as_str(),
            Some(metric.statistic),
            "case {case}: expected the published statistic {}, got {stored:?}",
            metric.statistic
        );
        assert_eq!(
            stored["period"].as_i64(),
            Some(ALARM_PERIOD_SECONDS),
            "case {case}: expected a five-minute period, got {stored:?}"
        );

        // The caller's own configuration, verbatim, and the state a new alarm
        // starts in.
        assert_eq!(
            stored["metricName"].as_str(),
            Some(metric.name),
            "case {case}: expected the requested metric, got {stored:?}"
        );
        assert_eq!(
            stored["comparisonOperator"].as_str(),
            Some(comparison_operator),
            "case {case}: expected the requested comparison operator, got {stored:?}"
        );
        assert_eq!(
            stored["threshold"].as_f64(),
            Some(threshold),
            "case {case}: expected the requested threshold, got {stored:?}"
        );
        assert_eq!(
            stored["evaluationPeriods"].as_i64(),
            Some(evaluation_periods),
            "case {case}: expected the requested evaluation periods, got {stored:?}"
        );
        assert_eq!(
            stored["state"].as_str(),
            Some(INSUFFICIENT_DATA),
            "case {case}: a new alarm has no data to evaluate, got {stored:?}"
        );
    }

    // Every metric of the catalog is stored, one alarm each, and the filter on
    // the monitored resource name reports them all.
    let (found, _) = alarms(&emulator, json!({"monitoredResourceName": WATCHED})).await;
    let mut reported = names(&found);
    reported.sort();
    let mut want: Vec<String> = METRICS
        .iter()
        .map(|metric| format!("lb-{}", metric.name))
        .collect();
    want.sort();
    assert_eq!(
        reported, want,
        "case the whole load balancer catalog: expected one alarm per published metric, \
         got {found:?}"
    );
}

// ---------------------------------------------------------------------------
// The Region decides which load balancer a name addresses
// ---------------------------------------------------------------------------

/// RULE: a load balancer belongs to ONE Region, so a name a request's Region
/// holds no load balancer under names no resource an alarm can monitor — while
/// the same name in the Region that holds it does.
#[tokio::test]
async fn a_load_balancer_of_another_region_is_not_a_monitored_resource() {
    let emulator = Emulator::new();
    let home = "us-west-2";
    let elsewhere = ["us-east-1", "eu-west-1"];
    create_load_balancer(&emulator, home, "edge-lb", 8080).await;

    for region in elsewhere {
        let case = format!("edge-lb watched from {region}");
        let (status, body) = emulator
            .call_in(
                Some(region),
                "PutAlarm",
                json!({
                    "alarmName": "edge-unhealthy",
                    "monitoredResourceName": "edge-lb",
                    "metricName": "UnhealthyHostCount",
                    "comparisonOperator": "GreaterThanThreshold",
                    "threshold": 1.0,
                    "evaluationPeriods": 4,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the put to be refused with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the {DOES_NOT_EXIST} envelope, got {body:?}"
        );
        let (stored, _) = alarms(&emulator, json!({})).await;
        assert!(
            stored.is_empty(),
            "case {case}: expected the refusal to store nothing, got {stored:?}"
        );
    }

    // The same request in the Region that holds the load balancer is served.
    let case = "edge-lb watched from us-west-2";
    emulator
        .ok_at(
            None,
            Some(home),
            "PutAlarm",
            json!({
                "alarmName": "edge-unhealthy",
                "monitoredResourceName": "edge-lb",
                "metricName": "UnhealthyHostCount",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 1.0,
                "evaluationPeriods": 4,
            }),
        )
        .await;
    let stored = alarm(&emulator, "edge-unhealthy", case).await;
    let balancer = load_balancer(&emulator, home, "edge-lb").await;
    assert_eq!(
        stored["monitoredResourceInfo"]["arn"].as_str(),
        balancer["arn"].as_str(),
        "case {case}: expected the arn GetLoadBalancer reports in {home}, got {stored:?}"
    );
    assert_eq!(
        stored["location"]["regionName"].as_str(),
        Some(home),
        "case {case}: expected the alarm in the load balancer's Region, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Metrics a load balancer does NOT publish to an alarm: the instance rows and
/// the relational database rows of
/// `smithy_query(query="doc",shape="PutAlarmRequest",member="metricName")`, plus
/// a member of the modelled `MetricName` enum that documentation lists for no
/// resource type at all.
const NOT_PUBLISHED_BY_A_LOAD_BALANCER: &[&str] = &[
    "CPUUtilization",
    "NetworkOut",
    "StatusCheckFailed_System",
    "DiskQueueLength",
    "DatabaseConnections",
    "FreeStorageSpace",
    "NetworkReceiveThroughput",
    "FreeableMemory",
];

/// RULE: each resource type publishes its OWN metrics, so a metric a load
/// balancer does not publish is refused as invalid input and stores nothing —
/// the same rule that refuses a load balancer metric on an instance.
#[tokio::test]
async fn a_metric_a_load_balancer_does_not_publish_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_load_balancer(&emulator, region, WATCHED, 80).await;

    for metric in NOT_PUBLISHED_BY_A_LOAD_BALANCER {
        let case = format!("{metric} on a load balancer");
        let (status, body) = emulator
            .call_in(
                Some(region),
                "PutAlarm",
                put_body("refused", metric, "GreaterThanThreshold", 1.0, 4),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the put to be refused with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
        let (stored, _) = alarms(&emulator, json!({})).await;
        assert!(
            stored.is_empty(),
            "case {case}: expected the refusal to store nothing, got {stored:?}"
        );
    }
}

/// RULE: one metric of one monitored resource carries at most the two alarms the
/// service documents — a load balancer metric like any other.
#[tokio::test]
async fn a_load_balancer_metric_carries_at_most_two_alarms() {
    let emulator = Emulator::new();
    let region = "eu-central-1";
    create_load_balancer(&emulator, region, WATCHED, 443).await;

    for index in 0..MAX_ALARMS_PER_METRIC {
        let case = format!("alarm {index} on RequestCount");
        let name = format!("requests-{index}");
        emulator
            .ok_at(
                None,
                Some(region),
                "PutAlarm",
                put_body(&name, "RequestCount", "GreaterThanThreshold", 10.0, 2),
            )
            .await;
        let stored = alarm(&emulator, &name, &case).await;
        assert_eq!(
            stored["metricName"].as_str(),
            Some("RequestCount"),
            "case {case}: expected the requested metric, got {stored:?}"
        );
    }

    let case = "a third alarm on RequestCount";
    let (status, body) = emulator
        .call_in(
            Some(region),
            "PutAlarm",
            put_body("requests-3", "RequestCount", "GreaterThanThreshold", 10.0, 2),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected the put to be refused with 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {case}: expected InvalidInputException, got {body:?}"
    );
    let (stored, _) = alarms(&emulator, json!({})).await;
    assert_eq!(
        stored.len(),
        MAX_ALARMS_PER_METRIC,
        "case {case}: expected the two alarms already stored to be left unchanged, got {stored:?}"
    );

    // Another metric of the same load balancer is unaffected: the bound is per
    // metric, not per resource.
    let case = "another metric of the same load balancer";
    emulator
        .ok_at(
            None,
            Some(region),
            "PutAlarm",
            put_body(
                "errors-0",
                "HTTPCode_LB_5XX_Count",
                "GreaterThanThreshold",
                1.0,
                2,
            ),
        )
        .await;
    let stored = alarm(&emulator, "errors-0", case).await;
    assert_eq!(
        stored["metricName"].as_str(),
        Some("HTTPCode_LB_5XX_Count"),
        "case {case}: expected the requested metric, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// The rest of the alarm surface
// ---------------------------------------------------------------------------

/// RULE: an alarm on a load balancer is an alarm, so the calls that move and
/// remove one reach it: `TestAlarm` puts it into the state the request names and
/// `DeleteAlarm` removes it, after which no alarm answers to that name.
#[tokio::test]
async fn a_load_balancer_alarm_is_tested_and_deleted_like_any_other() {
    let emulator = Emulator::new();
    let region = "ap-northeast-1";
    create_load_balancer(&emulator, region, WATCHED, 80).await;
    emulator
        .ok_at(
            None,
            Some(region),
            "PutAlarm",
            put_body(
                "lb-health",
                "HealthyHostCount",
                "LessThanThreshold",
                2.0,
                3,
            ),
        )
        .await;

    for state in ["ALARM", "OK", "INSUFFICIENT_DATA"] {
        let case = format!("tested into {state}");
        emulator
            .ok("TestAlarm", json!({"alarmName": "lb-health", "state": state}))
            .await;
        let stored = alarm(&emulator, "lb-health", &case).await;
        assert_eq!(
            stored["state"].as_str(),
            Some(state),
            "case {case}: expected the tested state, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["resourceType"].as_str(),
            Some(LOAD_BALANCER),
            "case {case}: expected a test to leave the watched resource alone, got {stored:?}"
        );
    }

    emulator
        .ok("DeleteAlarm", json!({"alarmName": "lb-health"}))
        .await;
    let (stored, _) = alarms(&emulator, json!({})).await;
    assert!(
        stored.is_empty(),
        "case deleted: expected no alarm to survive the delete, got {stored:?}"
    );
    let (status, body) = emulator
        .call("DeleteAlarm", json!({"alarmName": "lb-health"}))
        .await;
    assert_eq!(
        status, 400,
        "case deleted twice: expected the second delete to be refused, got {status} {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case deleted twice: expected the {DOES_NOT_EXIST} envelope, got {body:?}"
    );
}
