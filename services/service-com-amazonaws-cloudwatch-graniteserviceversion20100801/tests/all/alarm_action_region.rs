// SPDX-License-Identifier: CC0-1.0

//! An alarm's actions are executed in the Region the alarm lives in, so every alarm writer
//! refuses an action naming another Region with HTTP 400 `ValidationError` — "Invalid region
//! `<action region>` specified. Only `<alarm region>` is supported." — and writes nothing.
//!
//! The rows span what the rule generalises over rather than repeating one recorded request:
//! the Region the request is signed for, which of the three action-list members carries the
//! action, which Region the action names, and which of the action forms this operation
//! documents the ARN takes (SNS, Lambda in its three forms, Auto Scaling, SSM OpsItem, Amazon
//! Q, Simple Workflow, and the EC2 instance action, whose own precondition is reported first).
//! The same rule is driven through `PutCompositeAlarm` and `PutLogAlarm`, which carry the same
//! three action lists, and the accepted tables show a co-regional action — and the action ARN
//! that names NO Region — stored and read back through `DescribeAlarms`.

use crate::harness::{call_in_region, error_code};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Regions an alarm in this suite is created in: the recorded Region plus others, so the
/// refusal's "only" Region is the request's own and not a constant.
const ALARM_REGIONS: [&str; 3] = ["us-east-1", "us-west-2", "eu-central-1"];

/// The three action-list members every alarm writer carries; each one's documentation lists
/// the same action values.
const ACTION_MEMBERS: [&str; 3] = ["AlarmActions", "OKActions", "InsufficientDataActions"];

/// The account id the emulator mints its own ARNs under, used for the action ARNs too.
const ACCOUNT: &str = "000000000000";

/// The action ARN forms `PutMetricAlarm` documents, each with its Region segment filled from
/// `region`: "Each action is specified as an Amazon Resource Name (ARN). Valid values: …"
/// (`smithy_query(query="doc", shape="PutMetricAlarmInput", member="AlarmActions")`), which
/// lists the EC2, Auto Scaling, Lambda, SNS, SSM and Amazon Q forms. The EC2 instance actions
/// are exercised separately, because they carry a precondition of their own.
fn regional_action_forms(region: &str) -> Vec<(String, String)> {
    [
        ("an SNS topic", format!("arn:aws:sns:{region}:{ACCOUNT}:alarm-notifications")),
        (
            "the latest version of a Lambda function",
            format!("arn:aws:lambda:{region}:{ACCOUNT}:function:page-oncall"),
        ),
        (
            "a specific version of a Lambda function",
            format!("arn:aws:lambda:{region}:{ACCOUNT}:function:page-oncall:7"),
        ),
        (
            "an Auto Scaling policy",
            format!(
                "arn:aws:autoscaling:{region}:{ACCOUNT}:scalingPolicy:\
                 7bb0e3a5-0000-4000-8000-000000000000:autoScalingGroupName/web:policyName/out"
            ),
        ),
        (
            "an SSM OpsItem",
            format!("arn:aws:ssm:{region}:{ACCOUNT}:opsitem:3#CATEGORY=Availability"),
        ),
        (
            "an Amazon Q operational investigation group",
            format!("arn:aws:aiops:{region}:{ACCOUNT}:investigation-group:workloads"),
        ),
        (
            "a Simple Workflow action that is not an EC2 instance action",
            format!("arn:aws:swf:{region}:{ACCOUNT}:action/actions/AWS_SNS.Publish/1.0"),
        ),
    ]
    .into_iter()
    .map(|(form, arn)| (form.to_string(), arn))
    .collect()
}

/// The one documented action form whose ARN names NO Region at all — "arn:aws:ssm-incidents::
/// `account-id`:responseplan/`response-plan-name`" (`PutMetricAlarmInput$AlarmActions`) — so no
/// Region of it can disagree with the alarm's.
const REGIONLESS_ACTION: &str = "arn:aws:ssm-incidents::000000000000:responseplan/web-outage";

/// A Region other than `region`, for the action that must be refused.
fn another_region(region: &str) -> &'static str {
    ALARM_REGIONS
        .iter()
        .find(|candidate| **candidate != region)
        .expect("the Region table holds more than one Region")
}

/// The refusal CloudWatch answers for an action outside the alarm's Region, built from the
/// request: the Region the action named and the Region the alarm lives in.
fn refusal(action_region: &str, alarm_region: &str) -> String {
    format!("Invalid region {action_region} specified. Only {alarm_region} is supported.")
}

/// A `PutMetricAlarm` request for `name` carrying the recorded alarm's metric, with `members`
/// merged over it.
fn metric_alarm_request(name: &str, members: Value) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": "AWS/CloudFront",
        "MetricName": "4xxErrorRate",
        "Statistic": "Average",
        "Period": 300,
        "EvaluationPeriods": 1,
        "Threshold": 5.0,
        "ComparisonOperator": "GreaterThanThreshold",
    });
    merge(&mut request, members);
    request
}

/// A `PutCompositeAlarm` request for `name` over the child alarm `child`.
fn composite_alarm_request(name: &str, child: &str, members: Value) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "AlarmRule": format!("ALARM(\"{child}\")"),
    });
    merge(&mut request, members);
    request
}

/// A `PutLogAlarm` request for `name`: every required member and nothing else, plus `members`.
fn log_alarm_request(name: &str, members: Value) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "QueryResultsToEvaluate": 3,
        "QueryResultsToAlarm": 2,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "ScheduledQueryConfiguration": {
            "QueryString": "fields @message | filter @message like /ERROR/",
            "ScheduledQueryRoleARN": "arn:aws:iam::000000000000:role/cwl-scheduled-query",
            "AggregationExpression": "count(*)",
            "ScheduleConfiguration": {
                "ScheduleExpression": "rate(5 minutes)",
                "StartTimeOffset": 360,
            },
        },
    });
    merge(&mut request, members);
    request
}

/// Merge the members of `extra` over `request`, both JSON objects.
fn merge(request: &mut Value, extra: Value) {
    let (Value::Object(target), Value::Object(source)) = (request, extra) else {
        panic!("both the request and the overrides are JSON objects");
    };
    for (member, value) in source {
        target.insert(member, value);
    }
}

/// The alarms `DescribeAlarms` reports for exactly `name` under the output member `list`
/// (`MetricAlarms`, `CompositeAlarms` or `LogAlarms`), as seen from `region`.
async fn described(
    router: &MultiServiceRouter,
    region: &str,
    list: &str,
    name: &str,
) -> Vec<Value> {
    let (status, body) = call_in_region(
        router,
        region,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["MetricAlarm", "CompositeAlarm", "LogAlarm"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAlarms {name}: expected HTTP 400 never, got {status} with body {body}"
    );
    body.get(list)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One row of a refusal table: what is being driven, and in which Regions.
struct Row<'a> {
    /// The case the assertions name.
    case: &'a str,
    /// The Region the request is signed for, which is the alarm's own.
    alarm_region: &'a str,
    /// The Region the request's action names.
    action_region: &'a str,
    /// The alarm writer being driven.
    operation: &'a str,
    /// The `DescribeAlarms` output member the alarm would be reported under.
    list: &'a str,
    /// The alarm's name, which nothing may be stored under.
    name: &'a str,
}

/// Drive the row's operation with `request` and require the Region refusal naming its two
/// Regions, with no alarm of its name left behind under its output member.
async fn assert_refused(router: &MultiServiceRouter, row: &Row<'_>, request: &Value) {
    let Row {
        case,
        alarm_region,
        action_region,
        operation,
        list,
        name,
    } = row;
    let (status, body) = call_in_region(router, alarm_region, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "case {case:?}: expected the ValidationError code, got {body}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(refusal(action_region, alarm_region).as_str()),
        "case {case:?}: expected the Region refusal, got {body}"
    );
    let alarms = described(router, alarm_region, list, name).await;
    assert!(
        alarms.is_empty(),
        "case {case:?}: a refused request must store no alarm, got {alarms:?}"
    );
}

#[tokio::test]
async fn a_metric_alarm_action_in_another_region_is_refused_and_stores_nothing() {
    let router = crate::harness::build_router();

    let mut index = 0usize;
    for alarm_region in ALARM_REGIONS {
        let foreign = another_region(alarm_region);
        for member in ACTION_MEMBERS {
            for (form, arn) in regional_action_forms(foreign) {
                let name = format!("refused-{index}");
                index += 1;
                let case = format!("{form} in {foreign} on an alarm in {alarm_region}, {member}");
                let request = metric_alarm_request(&name, json!({member: [arn]}));
                assert_refused(
                    &router,
                    &Row {
                        case: &case,
                        alarm_region,
                        action_region: foreign,
                        operation: "PutMetricAlarm",
                        list: "MetricAlarms",
                        name: &name,
                    },
                    &request,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn a_foreign_region_action_is_refused_beside_co_regional_ones() {
    let router = crate::harness::build_router();

    for (index, alarm_region) in ALARM_REGIONS.iter().enumerate() {
        let foreign = another_region(alarm_region);
        let local = format!("arn:aws:sns:{alarm_region}:{ACCOUNT}:oncall");
        let distant = format!("arn:aws:sns:{foreign}:{ACCOUNT}:oncall-elsewhere");
        // Rows differ in WHERE the foreign action sits in the list and in how many
        // co-regional actions surround it: one bad action refuses the request wherever it is.
        let rows = [
            ("the foreign action first", vec![distant.clone(), local.clone()]),
            ("the foreign action last", vec![local.clone(), distant.clone()]),
            (
                "the foreign action between two co-regional ones",
                vec![local.clone(), distant.clone(), REGIONLESS_ACTION.to_string()],
            ),
        ];
        for (row, (case, actions)) in rows.into_iter().enumerate() {
            let name = format!("mixed-{index}-{row}");
            let request = metric_alarm_request(&name, json!({"AlarmActions": actions}));
            assert_refused(
                &router,
                &Row {
                    case: &format!("{case} on an alarm in {alarm_region}"),
                    alarm_region,
                    action_region: foreign,
                    operation: "PutMetricAlarm",
                    list: "MetricAlarms",
                    name: &name,
                },
                &request,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn an_ec2_instance_action_in_another_region_is_refused_too() {
    let router = crate::harness::build_router();

    // The EC2 instance action forms, whose own precondition this alarm satisfies (its metric
    // is dimensioned by `InstanceId`), so the Region rule is what speaks.
    for (index, alarm_region) in ALARM_REGIONS.iter().enumerate() {
        let foreign = another_region(alarm_region);
        let arns = [
            format!("arn:aws:automate:{foreign}:ec2:stop"),
            format!("arn:aws:automate:{foreign}:ec2:recover"),
            format!("arn:aws:swf:{foreign}:{ACCOUNT}:action/actions/AWS_EC2.InstanceId.Reboot/1.0"),
        ];
        for (row, arn) in arns.iter().enumerate() {
            let name = format!("ec2-refused-{index}-{row}");
            let request = metric_alarm_request(
                &name,
                json!({
                    "Namespace": "AWS/EC2",
                    "MetricName": "CPUUtilization",
                    "Dimensions": [{"Name": "InstanceId", "Value": "i-0123456789abcdef0"}],
                    "AlarmActions": [arn],
                }),
            );
            assert_refused(
                &router,
                &Row {
                    case: &format!("{arn} on an alarm in {alarm_region}"),
                    alarm_region,
                    action_region: foreign,
                    operation: "PutMetricAlarm",
                    list: "MetricAlarms",
                    name: &name,
                },
                &request,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_co_regional_action_is_stored_and_read_back() {
    let router = crate::harness::build_router();

    let mut index = 0usize;
    for alarm_region in ALARM_REGIONS {
        for (form, arn) in regional_action_forms(alarm_region) {
            // The action that names no Region rides along in every row: it is co-regional
            // with every alarm, whatever Region the alarm lives in.
            let actions = vec![arn.clone(), REGIONLESS_ACTION.to_string()];
            let name = format!("accepted-{index}");
            index += 1;
            let case = format!("{form} in {alarm_region} on an alarm in {alarm_region}");
            let request =
                metric_alarm_request(&name, json!({"AlarmActions": actions.clone()}));
            let (status, body) =
                call_in_region(&router, alarm_region, "PutMetricAlarm", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let alarms = described(&router, alarm_region, "MetricAlarms", &name).await;
            assert_eq!(
                alarms.len(),
                1,
                "case {case:?}: expected exactly the one alarm, got {alarms:?}"
            );
            assert_eq!(
                alarms[0].get("AlarmActions"),
                Some(&json!(actions)),
                "case {case:?}: expected the actions to round-trip, got {}",
                alarms[0]
            );
        }
    }
}

#[tokio::test]
async fn an_update_carrying_a_foreign_region_action_leaves_the_stored_actions() {
    let router = crate::harness::build_router();

    for (index, alarm_region) in ALARM_REGIONS.iter().enumerate() {
        let foreign = another_region(alarm_region);
        let local = format!("arn:aws:sns:{alarm_region}:{ACCOUNT}:oncall");
        let distant = format!("arn:aws:sns:{foreign}:{ACCOUNT}:oncall-elsewhere");
        let name = format!("configured-{index}");

        let created =
            metric_alarm_request(&name, json!({"AlarmActions": [local.clone()]}));
        let (status, body) =
            call_in_region(&router, alarm_region, "PutMetricAlarm", &created).await;
        assert_eq!(
            status, 200,
            "the co-regional alarm in {alarm_region}: expected HTTP 200, got {status} / {body}"
        );

        // The update adds the foreign action beside the valid one: refused, and the alarm
        // already there keeps the actions it had.
        let update = metric_alarm_request(
            &name,
            json!({"AlarmActions": [local.clone(), distant.clone()]}),
        );
        let case = format!("an update adding an action in {foreign} in {alarm_region}");
        let (update_status, update_body) =
            call_in_region(&router, alarm_region, "PutMetricAlarm", &update).await;
        assert_eq!(
            update_status, 400,
            "case {case:?}: expected HTTP 400, got {update_status} with body {update_body}"
        );
        assert_eq!(
            error_code(&update_body),
            "ValidationError",
            "case {case:?}: expected the ValidationError code, got {update_body}"
        );
        assert_eq!(
            update_body.get("message").and_then(Value::as_str),
            Some(refusal(foreign, alarm_region).as_str()),
            "case {case:?}: expected the Region refusal, got {update_body}"
        );
        let alarms = described(&router, alarm_region, "MetricAlarms", &name).await;
        assert_eq!(
            alarms.len(),
            1,
            "case {case:?}: the configured alarm must still be there, got {alarms:?}"
        );
        assert_eq!(
            alarms[0].get("AlarmActions"),
            Some(&json!([local])),
            "case {case:?}: the refused update must leave the stored actions, got {}",
            alarms[0]
        );
    }
}

#[tokio::test]
async fn the_composite_and_log_alarm_writers_refuse_a_foreign_region_action() {
    let router = crate::harness::build_router();

    for (index, alarm_region) in ALARM_REGIONS.iter().enumerate() {
        let foreign = another_region(alarm_region);
        let local = format!("arn:aws:sns:{alarm_region}:{ACCOUNT}:oncall");
        let distant = format!("arn:aws:lambda:{foreign}:{ACCOUNT}:function:page-oncall");

        // A child alarm for the composite alarm's rule, configured co-regionally.
        let child = format!("child-{index}");
        let (status, body) = call_in_region(
            &router,
            alarm_region,
            "PutMetricAlarm",
            &metric_alarm_request(&child, json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "the child alarm in {alarm_region}: expected HTTP 200, got {status} / {body}"
        );

        for member in ACTION_MEMBERS {
            let composite = format!("composite-{index}-{member}");
            assert_refused(
                &router,
                &Row {
                    case: &format!("PutCompositeAlarm with {member} in {foreign}"),
                    alarm_region,
                    action_region: foreign,
                    operation: "PutCompositeAlarm",
                    list: "CompositeAlarms",
                    name: &composite,
                },
                &composite_alarm_request(&composite, &child, json!({member: [distant]})),
            )
            .await;

            let log = format!("log-{index}-{member}");
            assert_refused(
                &router,
                &Row {
                    case: &format!("PutLogAlarm with {member} in {foreign}"),
                    alarm_region,
                    action_region: foreign,
                    operation: "PutLogAlarm",
                    list: "LogAlarms",
                    name: &log,
                },
                &log_alarm_request(&log, json!({member: [distant]})),
            )
            .await;
        }

        // The same writers accept the co-regional action, and report it back.
        let composite = format!("composite-ok-{index}");
        let (composite_status, composite_body) = call_in_region(
            &router,
            alarm_region,
            "PutCompositeAlarm",
            &composite_alarm_request(&composite, &child, json!({"AlarmActions": [local.clone()]})),
        )
        .await;
        assert_eq!(
            composite_status, 200,
            "the co-regional composite alarm in {alarm_region}: expected HTTP 200, got \
             {composite_status} / {composite_body}"
        );
        let stored = described(&router, alarm_region, "CompositeAlarms", &composite).await;
        assert_eq!(
            stored.len(),
            1,
            "the co-regional composite alarm in {alarm_region}: expected one alarm, got {stored:?}"
        );
        assert_eq!(
            stored[0].get("AlarmActions"),
            Some(&json!([local.clone()])),
            "the co-regional composite alarm in {alarm_region}: expected the action to \
             round-trip, got {}",
            stored[0]
        );

        let log = format!("log-ok-{index}");
        let (log_status, log_body) = call_in_region(
            &router,
            alarm_region,
            "PutLogAlarm",
            &log_alarm_request(&log, json!({"AlarmActions": [local.clone()]})),
        )
        .await;
        assert_eq!(
            log_status, 200,
            "the co-regional log alarm in {alarm_region}: expected HTTP 200, got {log_status} / \
             {log_body}"
        );
        let stored_log = described(&router, alarm_region, "LogAlarms", &log).await;
        assert_eq!(
            stored_log.len(),
            1,
            "the co-regional log alarm in {alarm_region}: expected one alarm, got {stored_log:?}"
        );
        assert_eq!(
            stored_log[0].get("AlarmActions"),
            Some(&json!([local])),
            "the co-regional log alarm in {alarm_region}: expected the action to round-trip, \
             got {}",
            stored_log[0]
        );
    }
}
