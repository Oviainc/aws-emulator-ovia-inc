// SPDX-License-Identifier: CC0-1.0

//! The EC2-instance-action precondition on `PutMetricAlarm`: an EC2 instance action is
//! accepted only on an alarm whose every watched metric is dimensioned by `InstanceId`,
//! and any other request carrying one is refused with HTTP 400 under the request-validation
//! class and
//! stores nothing.
//!
//! The rows span what the rule generalises over — which of the four EC2 actions, which
//! action-list member, the `automate` form and the Simple Workflow form, the partition and
//! region, the `Dimensions` form (absent / empty / other names / `InstanceId` present) and
//! the `Metrics` query-array form — so the recorded `automate:...:ec2:stop` case with no
//! dimensions is one row among many. Every alarm an accepted row creates is read back
//! through `DescribeAlarms`, and every refused one is shown to be absent from it.
//!
//! Every ACCEPTED row's actions name the Region its request is signed for, because an action
//! in another Region is refused on its own account (`alarm_action_region.rs`); the refused
//! rows show this precondition is reported first, including for an action in another partition
//! and Region.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};

/// The refusal text CloudWatch answers for this precondition — one fixed sentence for every
/// EC2 instance action on an alarm that does not monitor an EC2 instance.
const EC2_ACTION_REFUSAL: &str = "Invalid use of EC2 action. An EC2 action can only be used \
                                  if the alarm is monitoring an EC2 instance.";

/// The EC2 instance actions this operation's model documents, in both forms it lists:
/// "EC2 actions: arn:aws:automate:region:ec2:stop arn:aws:automate:region:ec2:terminate
/// arn:aws:automate:region:ec2:reboot arn:aws:automate:region:ec2:recover
/// arn:aws:swf:region:account-id:action/actions/AWS_EC2.InstanceId.Stop/1.0 …"
/// (`smithy_query(query="doc", shape="PutMetricAlarmInput", member="AlarmActions")`).
const EC2_ACTION_ARNS: [&str; 8] = [
    "arn:aws:automate:us-east-1:ec2:stop",
    "arn:aws:automate:us-east-1:ec2:terminate",
    "arn:aws:automate:us-east-1:ec2:reboot",
    "arn:aws:automate:us-east-1:ec2:recover",
    "arn:aws:swf:us-east-1:123456789012:action/actions/AWS_EC2.InstanceId.Stop/1.0",
    "arn:aws:swf:us-east-1:123456789012:action/actions/AWS_EC2.InstanceId.Terminate/1.0",
    "arn:aws:swf:us-east-1:123456789012:action/actions/AWS_EC2.InstanceId.Reboot/1.0",
    "arn:aws:swf:us-east-1:123456789012:action/actions/AWS_EC2.InstanceId.Recover/1.0",
];

/// The three action-list members an alarm action can ride in; each one's documentation
/// lists the same EC2 action values.
const ACTION_MEMBERS: [&str; 3] = ["AlarmActions", "OKActions", "InsufficientDataActions"];

/// A `PutMetricAlarm` request for `name`: the static-threshold members every row shares,
/// with the row's own members merged over them.
fn put_request(name: &str, members: &Value) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "ComparisonOperator": "GreaterThanThreshold",
        "EvaluationPeriods": 1,
        "Period": 60,
        "Statistic": "Average",
        "Threshold": 80.0,
    });
    let object = request
        .as_object_mut()
        .expect("the base request is an object");
    for (member, value) in members
        .as_object()
        .expect("a row's members are a JSON object")
    {
        object.insert(member.clone(), value.clone());
    }
    // "If you use the `Metrics` parameter, you cannot include the `Namespace`, `MetricName`,
    // `Dimensions`, `Period`, `Unit`, `Statistic`, or `ExtendedStatistic` parameters of
    // `PutMetricAlarm` in the same operation."
    // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Metrics")`) — a row that
    // states its series as a query array carries none of the single-metric members the base
    // request above shares, since the array states the period and the statistic itself.
    if object.contains_key("Metrics") {
        for member in [
            "Namespace",
            "MetricName",
            "Dimensions",
            "Period",
            "Unit",
            "Statistic",
            "ExtendedStatistic",
        ] {
            object.remove(member);
        }
    }
    request
}

/// One `MetricDataQuery` retrieving `metric` under `id`.
fn metric_query(id: &str, dimensions: Value) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "Dimensions": dimensions,
            },
            "Period": 60,
            "Stat": "Average",
        },
        "ReturnData": true,
    })
}

/// The same query FEEDING the one the alarm watches rather than being it: exactly one query of
/// a `Metrics` array returns data ("For all other metrics and expressions in the same
/// PutMetricAlarm operation, specify ReturnData as False.", `MetricDataQuery$ReturnData`), so
/// an array holding a second metric states that one this way.
fn feeding_query(id: &str, dimensions: Value) -> Value {
    let mut query = metric_query(id, dimensions);
    query["ReturnData"] = json!(false);
    query
}

/// The alarms `DescribeAlarms` reports for exactly `name`.
async fn described(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Vec<Value> {
    let body = ok(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    body.get("MetricAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn an_ec2_instance_action_is_refused_when_the_metric_is_not_per_instance() {
    let router = build_router();

    // Rows differ in the action, the action-list member, the ARN form, the partition and
    // region, and how the metric is identified — none of them names an instance.
    let mut cases: Vec<(String, Value)> = Vec::new();

    // Every documented EC2 action ARN, in every action-list member, on the recorded
    // metric shape (`AWS/EC2` `CPUUtilization` with no dimensions at all).
    for arn in EC2_ACTION_ARNS {
        for member in ACTION_MEMBERS {
            cases.push((
                format!("{arn} in {member} with no Dimensions member"),
                json!({
                    "Namespace": "AWS/EC2",
                    "MetricName": "CPUUtilization",
                    member: [arn],
                }),
            ));
        }
    }

    cases.extend([
        (
            "an empty Dimensions list".to_string(),
            json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "Dimensions": [],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:stop"],
            }),
        ),
        (
            "dimensions that do not include InstanceId".to_string(),
            json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "Dimensions": [
                    {"Name": "AutoScalingGroupName", "Value": "web"},
                    {"Name": "ImageId", "Value": "ami-0123456789abcdef0"},
                ],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:terminate"],
            }),
        ),
        (
            "a dimension whose NAME merely contains InstanceId".to_string(),
            json!({
                "Namespace": "Custom",
                "MetricName": "Depth",
                "Dimensions": [{"Name": "InstanceIdentity", "Value": "i-0123456789abcdef0"}],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:reboot"],
            }),
        ),
        (
            "an EC2 action in another partition and region".to_string(),
            json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "AlarmActions": ["arn:aws-us-gov:automate:us-gov-west-1:ec2:stop"],
            }),
        ),
        (
            "an EC2 action listed after a non-EC2 action".to_string(),
            json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "AlarmActions": [
                    "arn:aws:sns:us-east-1:123456789012:ops",
                    "arn:aws:automate:us-east-1:ec2:recover",
                ],
            }),
        ),
        (
            "a Metrics query whose metric has no InstanceId dimension".to_string(),
            json!({
                "Metrics": [metric_query("m1", json!([]))],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:stop"],
            }),
        ),
        (
            "a Metrics array where only one of two metrics is per-instance".to_string(),
            json!({
                "Metrics": [
                    metric_query("m1", json!([{"Name": "InstanceId", "Value": "i-0123456789abcdef0"}])),
                    metric_query("m2", json!([{"Name": "AutoScalingGroupName", "Value": "web"}])),
                ],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:terminate"],
            }),
        ),
        (
            "a Metrics array that only computes an expression".to_string(),
            json!({
                "Metrics": [{"Id": "e1", "Expression": "100", "ReturnData": true}],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:stop"],
            }),
        ),
    ]);

    for (index, (case, members)) in cases.iter().enumerate() {
        let name = format!("refused-{index}");
        let request = put_request(&name, members);
        let (status, body) = call(&router, "PutMetricAlarm", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the request-validation class, got {body}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(EC2_ACTION_REFUSAL),
            "case {case:?}: expected the EC2-action refusal message, got {body}"
        );
        let alarms = described(&router, &name).await;
        assert!(
            alarms.is_empty(),
            "case {case:?}: a refused request must store no alarm, got {alarms:?}"
        );
    }
}

#[tokio::test]
async fn an_alarm_whose_metrics_name_an_instance_keeps_its_ec2_actions() {
    let router = build_router();

    // Rows differ in the action, the ARN form, the action-list member and how the
    // per-instance metric is identified (top-level Dimensions, in either order, or a
    // Metrics array). The last rows carry an action that is NOT an EC2 instance action, so
    // the precondition does not apply to them at all.
    let mut cases: Vec<(String, Value)> = Vec::new();

    for arn in EC2_ACTION_ARNS {
        for member in ACTION_MEMBERS {
            cases.push((
                format!("{arn} in {member} on an InstanceId-dimensioned metric"),
                json!({
                    "Namespace": "AWS/EC2",
                    "MetricName": "CPUUtilization",
                    "Dimensions": [{"Name": "InstanceId", "Value": "i-0123456789abcdef0"}],
                    member: [arn],
                }),
            ));
        }
    }

    cases.extend([
        (
            "InstanceId listed after another dimension".to_string(),
            json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "Dimensions": [
                    {"Name": "AutoScalingGroupName", "Value": "web"},
                    {"Name": "InstanceId", "Value": "i-0123456789abcdef0"},
                ],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:stop"],
            }),
        ),
        (
            "every metric of a Metrics array per-instance".to_string(),
            json!({
                "Metrics": [
                    metric_query("m1", json!([{"Name": "InstanceId", "Value": "i-0123456789abcdef0"}])),
                    feeding_query("m2", json!([{"Name": "InstanceId", "Value": "i-0fedcba9876543210"}])),
                ],
                "AlarmActions": ["arn:aws:automate:us-east-1:ec2:reboot"],
            }),
        ),
        (
            "an SNS action on a metric with no InstanceId dimension".to_string(),
            json!({
                "Namespace": "AWS/EC2",
                "MetricName": "CPUUtilization",
                "AlarmActions": ["arn:aws:sns:us-east-1:123456789012:ops"],
            }),
        ),
        (
            "an Auto Scaling action on a metric with no InstanceId dimension".to_string(),
            json!({
                "Namespace": "AWS/ApplicationELB",
                "MetricName": "RequestCount",
                "Dimensions": [{"Name": "LoadBalancer", "Value": "app/web/0123456789abcdef"}],
                "OKActions": [
                    "arn:aws:autoscaling:us-east-1:123456789012:scalingPolicy:\
                     7bb0e3a5-0000-4000-8000-000000000000:autoScalingGroupName/web:policyName/out"
                ],
            }),
        ),
        (
            "an SWF action that is not an EC2 instance action".to_string(),
            json!({
                "Namespace": "Custom",
                "MetricName": "Depth",
                "AlarmActions": [
                    "arn:aws:swf:us-east-1:123456789012:action/actions/AWS_SNS.Publish/1.0"
                ],
            }),
        ),
        (
            "no actions at all and no dimensions".to_string(),
            json!({"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}),
        ),
    ]);

    for (index, (case, members)) in cases.iter().enumerate() {
        let name = format!("accepted-{index}");
        let request = put_request(&name, members);
        let (status, body) = call(&router, "PutMetricAlarm", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // Round trip: the alarm the request asked for is the one the reader reports.
        let alarms = described(&router, &name).await;
        assert_eq!(
            alarms.len(),
            1,
            "case {case:?}: expected exactly the one alarm, got {alarms:?}"
        );
        let alarm = &alarms[0];
        assert_eq!(
            alarm.get("AlarmName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the alarm name to round-trip, got {alarm}"
        );
        for member in ACTION_MEMBERS
            .iter()
            .chain(["Dimensions", "Metrics", "Namespace", "MetricName"].iter())
        {
            if let Some(sent) = request.get(*member) {
                assert_eq!(
                    alarm.get(*member),
                    Some(sent),
                    "case {case:?}: expected {member} to round-trip as {sent}, got {alarm}"
                );
            }
        }
    }
}
