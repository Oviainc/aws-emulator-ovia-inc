// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the CloudWatch alarm a reported health check carries,
//! through `GetHealthCheck` and the other readers of a health check.
//!
//! The rule under test: a health check of the alarm-reading type
//! (`CLOUDWATCH_METRIC`) reports, as its `CloudWatchAlarmConfiguration`, the
//! configuration of the CloudWatch alarm its `AlarmIdentifier` names — the metric
//! that alarm watches, the statistic and period it watches it over, and the
//! evaluation periods, threshold and comparison its evaluation compares by — read
//! out of CloudWatch's own alarm store, with nothing reported for a health check
//! that monitors no alarm the account holds or whose alarm this API's
//! `CloudWatchAlarmConfiguration` cannot describe.
//!
//! So the table below drives the rule over a SET of alarms rather than one: three
//! alarms differing in metric, namespace, statistic, period, evaluation periods,
//! threshold, comparison operator and dimension count (none, one, two), an alarm
//! on an extended statistic and one comparing by an operator outside this
//! service's own `ComparisonOperator` value list, a health check naming an alarm
//! nobody created, and a health check of a type that monitors no alarm at all.
//! Every row is asserted through `GetHealthCheck`, and through the create and the
//! listing that report the same health check, so the three readers cannot
//! disagree; `GetHealthCheck` is a report, so the table also asserts that reading
//! it twice says the same thing and leaves the account as it was.
//!
//! Route 53 reads the alarm from the CloudWatch emulator's own store, so this
//! suite mounts both services on ONE engine and writes each alarm through
//! `PutMetricAlarm` — the operation a client creates it with — rather than
//! reaching into either store.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::{
    COMPARISON_OPERATOR_VALUES, STATISTIC_VALUES,
};

use crate::health_check_listing::list_health_checks;
use crate::health_checks::{
    create_health_check, get_health_check, health_check_count, health_check_id,
};
use crate::hosted_zone_records::Reply;

/// The CloudWatch operation prefix the `x-amz-target` header names, from that
/// service's own model (`GraniteServiceVersion20100801`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The Region every call below is addressed to.
const REGION: &str = "us-east-1";

/// A router with Route 53 and CloudWatch mounted on ONE shared engine.
///
/// The alarm a health check monitors belongs to CloudWatch, so the rule under
/// test is only observable with both services reachable through the same engine —
/// which is how the router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Create one CloudWatch alarm named `name` out of `members`, requiring the call
/// to succeed.
async fn put_metric_alarm(router: &MultiServiceRouter, case: &str, name: &str, members: &Value) {
    let mut body = members.clone();
    body["AlarmName"] = json!(name);
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(http::header::HOST, format!("monitoring.{REGION}.amazonaws.com"))
        .header(http::header::CONTENT_TYPE, "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.PutMetricAlarm"),
        )
        // The SigV4 credential scope is what routes the request to the
        // `monitoring` signing name.
        .header(
            http::header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the alarm request serializes"),
        ))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from PutMetricAlarm for {name:?}, got {status} with body \
         {answered}"
    );
}

/// The `CloudWatchAlarmConfiguration` of a reported health check.
#[derive(Debug, Clone, PartialEq)]
struct Configuration {
    evaluation_periods: i64,
    threshold: f64,
    comparison_operator: String,
    period: i64,
    metric_name: String,
    namespace: String,
    statistic: String,
    /// `(Name, Value)` of each dimension, in the order the response lists them.
    dimensions: Vec<(String, String)>,
}

/// The members of `CloudWatchAlarmConfiguration` the model makes required, so a
/// reply carrying the structure at all has to carry every one of them.
const REQUIRED_MEMBERS: [&str; 7] = [
    "EvaluationPeriods",
    "Threshold",
    "ComparisonOperator",
    "Period",
    "MetricName",
    "Namespace",
    "Statistic",
];

/// The alarm configuration one reply reports, or `None` when it reports none.
fn reported(reply: &Reply, case: &str) -> Option<Configuration> {
    let text = |member: &str| {
        reply
            .first(&format!("CloudWatchAlarmConfiguration/{member}"))
            .map(str::to_string)
    };
    let present: Vec<&str> = REQUIRED_MEMBERS
        .iter()
        .copied()
        .filter(|member| text(member).is_some())
        .collect();
    if present.is_empty() {
        return None;
    }
    assert_eq!(
        present.len(),
        REQUIRED_MEMBERS.len(),
        "case {case:?}: every member of CloudWatchAlarmConfiguration is required, so a reported \
         configuration must carry all of {REQUIRED_MEMBERS:?}, got {present:?}; body {}",
        reply.body
    );
    let required = |member: &str| text(member).unwrap_or_default();
    let number = |member: &str| -> i64 {
        let value = required(member);
        value.parse::<i64>().unwrap_or_else(|_| {
            panic!(
                "case {case:?}: CloudWatchAlarmConfiguration/{member} must be an integer, got \
                 {value:?}; body {}",
                reply.body
            )
        })
    };
    let threshold = required("Threshold");
    let names = reply.all("Dimension/Name");
    let values = reply.all("Dimension/Value");
    assert_eq!(
        names.len(),
        values.len(),
        "case {case:?}: every reported dimension must carry a Name and a Value, got {} name(s) \
         and {} value(s); body {}",
        names.len(),
        values.len(),
        reply.body
    );
    Some(Configuration {
        evaluation_periods: number("EvaluationPeriods"),
        threshold: threshold.parse::<f64>().unwrap_or_else(|_| {
            panic!(
                "case {case:?}: CloudWatchAlarmConfiguration/Threshold must be a number, got \
                 {threshold:?}; body {}",
                reply.body
            )
        }),
        comparison_operator: required("ComparisonOperator"),
        period: number("Period"),
        metric_name: required("MetricName"),
        namespace: required("Namespace"),
        statistic: required("Statistic"),
        dimensions: names
            .into_iter()
            .zip(values)
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    })
}

/// One row of the table: a CloudWatch alarm, the health check that monitors it,
/// and the configuration that health check must then report.
struct Row {
    case: &'static str,
    /// The `PutMetricAlarm` members the alarm is created with, or `None` for a
    /// row whose health check names an alarm nobody creates.
    alarm: Option<Value>,
    /// The `HealthCheckConfig` members beyond the alarm this row's health check
    /// monitors, used as given when the row names no alarm at all.
    config: Option<&'static str>,
    /// The configuration the reported health check must carry.
    expected: Option<Configuration>,
}

/// A reported dimension list, as a row spells it out.
fn dimensions(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// The table: the alarms a health check can monitor, and what each is reported as.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an alarm on an undimensioned metric",
            alarm: Some(json!({
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
            })),
            config: None,
            expected: Some(Configuration {
                evaluation_periods: 1,
                threshold: 1.0,
                comparison_operator: "GreaterThanThreshold".to_string(),
                period: 60,
                metric_name: "Latency".to_string(),
                namespace: "MyApp".to_string(),
                statistic: "Average".to_string(),
                dimensions: dimensions(&[]),
            }),
        },
        Row {
            case: "an alarm on a metric with one dimension",
            alarm: Some(json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "Statistic": "Sum",
                "Period": 300,
                "EvaluationPeriods": 3,
                "Threshold": 0.5,
                "ComparisonOperator": "GreaterThanOrEqualToThreshold",
                "Dimensions": [{"Name": "InstanceId", "Value": "i-0123456789abcdef0"}],
            })),
            config: None,
            expected: Some(Configuration {
                evaluation_periods: 3,
                threshold: 0.5,
                comparison_operator: "GreaterThanOrEqualToThreshold".to_string(),
                period: 300,
                metric_name: "CPUUtilization".to_string(),
                namespace: "AWS/EC2".to_string(),
                statistic: "Sum".to_string(),
                dimensions: dimensions(&[("InstanceId", "i-0123456789abcdef0")]),
            }),
        },
        Row {
            case: "an alarm on a metric with two dimensions, compared downwards",
            alarm: Some(json!({
                "Namespace": "AWS/ApplicationELB",
                "MetricName": "HealthyHostCount",
                "Statistic": "Minimum",
                "Period": 120,
                "EvaluationPeriods": 2,
                "Threshold": -2.5,
                "ComparisonOperator": "LessThanThreshold",
                "Dimensions": [
                    {"Name": "LoadBalancer", "Value": "app/front/0123456789abcdef"},
                    {"Name": "TargetGroup", "Value": "targetgroup/web/abcdef0123456789"},
                ],
            })),
            config: None,
            expected: Some(Configuration {
                evaluation_periods: 2,
                threshold: -2.5,
                comparison_operator: "LessThanThreshold".to_string(),
                period: 120,
                metric_name: "HealthyHostCount".to_string(),
                namespace: "AWS/ApplicationELB".to_string(),
                statistic: "Minimum".to_string(),
                dimensions: dimensions(&[
                    ("LoadBalancer", "app/front/0123456789abcdef"),
                    ("TargetGroup", "targetgroup/web/abcdef0123456789"),
                ]),
            }),
        },
        Row {
            // "you must specify either Statistic or ExtendedStatistic but not
            // both" (CloudWatch's `PutMetricAlarm`), and `Statistic` carries only
            // the five values below — a percentile is none of them, so there is
            // no configuration of this alarm to report.
            case: "an alarm on an extended statistic",
            alarm: Some(json!({
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "ExtendedStatistic": "p90",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
            })),
            config: None,
            expected: None,
        },
        Row {
            // CloudWatch compares an anomaly-detection alarm by an operator
            // `ComparisonOperator` does not carry, so this alarm's evaluation
            // cannot be described either.
            case: "an alarm compared by an operator this API does not carry",
            alarm: Some(json!({
                "Namespace": "MyApp",
                "MetricName": "Faults",
                "Statistic": "SampleCount",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "LessThanLowerOrGreaterThanUpperThreshold",
            })),
            config: None,
            expected: None,
        },
        Row {
            case: "a health check monitoring an alarm nobody created",
            alarm: None,
            config: None,
            expected: None,
        },
        Row {
            case: "a health check that monitors no alarm at all",
            alarm: None,
            config: Some(
                "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                 <ResourcePath>/health</ResourcePath>",
            ),
            expected: None,
        },
    ]
}

/// The `HealthCheckConfig` of a health check monitoring the alarm named `alarm`.
fn alarm_reader(alarm: &str) -> String {
    format!(
        "<Type>CLOUDWATCH_METRIC</Type>\
         <AlarmIdentifier><Region>{REGION}</Region><Name>{alarm}</Name></AlarmIdentifier>\
         <InsufficientDataHealthStatus>LastKnownStatus</InsufficientDataHealthStatus>"
    )
}

/// The health check a row creates: its alarm written first where it has one, and
/// the id the create minted.
async fn create_row(router: &MultiServiceRouter, row: &Row, index: usize) -> (String, Reply) {
    let alarm_name = format!("alarm-{index}");
    if let Some(members) = &row.alarm {
        put_metric_alarm(router, row.case, &alarm_name, members).await;
    }
    let config = row
        .config
        .map(str::to_string)
        .unwrap_or_else(|| alarm_reader(&alarm_name));
    let created = create_health_check(router, &format!("reference-{index}"), &config).await;
    let id = health_check_id(&created, row.case);
    (id, created)
}

#[tokio::test]
async fn a_health_check_reports_the_configuration_of_the_alarm_it_monitors() {
    let router = emulator();
    let rows = rows();
    let mut ids = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let (id, created) = create_row(&router, row, index).await;
        let case = row.case;

        // The create reports the health check it stored, so it answers with the
        // same alarm the read below does.
        assert_eq!(
            reported(&created, case),
            row.expected,
            "case {case:?}: CreateHealthCheck must report the alarm's configuration; body {}",
            created.body
        );

        let read = get_health_check(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetHealthCheck (501 = still scaffolded), got {} \
             with body {}",
            read.status, read.body
        );
        assert_eq!(
            reported(&read, case),
            row.expected,
            "case {case:?}: GetHealthCheck must report the alarm's configuration; body {}",
            read.body
        );

        // A report, so reading it again says the same thing.
        let again = get_health_check(&router, &id).await;
        assert_eq!(
            reported(&again, case),
            row.expected,
            "case {case:?}: a second GetHealthCheck must report the same configuration; body {}",
            again.body
        );
        ids.push(id);
    }

    // Nothing above created a health check beyond the one per row, so the reads
    // left the account exactly as the creates did.
    let count = health_check_count(&router, "after every row was read").await;
    assert_eq!(
        count,
        rows.len() as i64,
        "case {:?}: the account must hold one health check per row, got {count}",
        "every row created"
    );

    // And the listing, which projects the same health checks, reports each row's
    // configuration too.
    let listed = list_health_checks(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListHealthChecks, got {} with body {}",
        listed.status, listed.body
    );
    let listed_ids: Vec<String> = listed
        .all("HealthCheck/Id")
        .iter()
        .map(|id| (*id).to_string())
        .collect();
    assert_eq!(
        listed_ids.len(),
        rows.len(),
        "the listing must report one health check per row, got {listed_ids:?}; body {}",
        listed.body
    );
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One health check per listing read, so the row's own configuration is
        // the one asserted rather than another row's.
        let single = get_health_check(&router, &ids[index]).await;
        assert!(
            listed_ids.contains(&ids[index]),
            "case {case:?}: the listing must report {:?}, got {listed_ids:?}; body {}",
            ids[index],
            listed.body
        );
        assert_eq!(
            reported(&single, case),
            row.expected,
            "case {case:?}: the health check the listing names must read back with the same \
             configuration; body {}",
            single.body
        );
    }
}

#[tokio::test]
async fn an_update_reports_the_alarm_the_check_monitors_after_it() {
    let router = emulator();
    let rows = rows();
    // The two rows that report a configuration, so the update below moves the
    // health check from one alarm to another and from an alarm to none.
    let first = &rows[0];
    let second = &rows[1];
    put_metric_alarm(&router, first.case, "update-first", first.alarm.as_ref().expect("row 0 has an alarm")).await;
    put_metric_alarm(
        &router,
        second.case,
        "update-second",
        second.alarm.as_ref().expect("row 1 has an alarm"),
    )
    .await;

    let created =
        create_health_check(&router, "updated-reference", &alarm_reader("update-first")).await;
    let id = health_check_id(&created, "the health check that is updated");
    assert_eq!(
        reported(&created, first.case),
        first.expected,
        "the create must report the first alarm's configuration; body {}",
        created.body
    );

    // Each row: the `AlarmIdentifier` the update names, and the configuration the
    // health check must report after it.
    let moves: [(&str, &str, Option<Configuration>); 3] = [
        ("the second alarm", "update-second", second.expected.clone()),
        ("an alarm nobody created", "update-missing", None),
        ("back to the first alarm", "update-first", first.expected.clone()),
    ];
    for (case, alarm, expected) in moves {
        let body = format!(
            "<UpdateHealthCheckRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <AlarmIdentifier><Region>{REGION}</Region><Name>{alarm}</Name></AlarmIdentifier>\
             </UpdateHealthCheckRequest>"
        );
        let updated = crate::hosted_zone_records::call(
            &router,
            "POST",
            &format!("/2013-04-01/healthcheck/{id}"),
            &body,
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            reported(&updated, case),
            expected,
            "case {case:?}: UpdateHealthCheck must report the alarm the check monitors after it; \
             body {}",
            updated.body
        );
        let read = get_health_check(&router, &id).await;
        assert_eq!(
            reported(&read, case),
            expected,
            "case {case:?}: GetHealthCheck must report what the update left behind; body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn every_reported_statistic_and_comparison_is_one_the_model_carries() {
    let router = emulator();
    // The model's own value lists, so an alarm whose statistic or comparison this
    // API cannot express is never reported as one it can
    // (`smithy_query(query="shape",shape="com.amazonaws.route53#Statistic")` and
    // `…#ComparisonOperator`).
    for (index, statistic) in STATISTIC_VALUES.iter().enumerate() {
        for (position, comparison) in COMPARISON_OPERATOR_VALUES.iter().enumerate() {
            let case = format!("an alarm on {statistic} compared by {comparison}");
            let alarm = format!("catalog-{index}-{position}");
            put_metric_alarm(
                &router,
                &case,
                &alarm,
                &json!({
                    "Namespace": "MyApp",
                    "MetricName": "Latency",
                    "Statistic": statistic,
                    "Period": 60,
                    "EvaluationPeriods": 1,
                    "Threshold": 1.0,
                    "ComparisonOperator": comparison,
                }),
            )
            .await;
            let created =
                create_health_check(&router, &alarm, &alarm_reader(&alarm)).await;
            let id = health_check_id(&created, &case);
            let read = get_health_check(&router, &id).await;
            let configuration = reported(&read, &case).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the health check must report the alarm's configuration; body \
                     {}",
                    read.body
                )
            });
            assert_eq!(
                configuration.statistic,
                (*statistic).to_string(),
                "case {case:?}: expected the alarm's statistic, got {:?}; body {}",
                configuration.statistic,
                read.body
            );
            assert_eq!(
                configuration.comparison_operator,
                (*comparison).to_string(),
                "case {case:?}: expected the alarm's comparison, got {:?}; body {}",
                configuration.comparison_operator,
                read.body
            );
        }
    }
}
