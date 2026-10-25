// SPDX-License-Identifier: CC0-1.0

//! An alarm's SNS action fans its notification out to the subscribed queues that GRANT the
//! send, end to end over the wire through the real `MultiServiceRouter` with CloudWatch, SNS
//! and SQS on one engine — so what a client sees is what is asserted: the depth of each
//! queue.
//!
//! The RULE asserted here: a notification reaches a subscribed SQS queue only where that
//! queue's own resource policy allows the SNS service principal `sqs:SendMessage` on it, so
//! one alarm transition fanned to two subscribers of one topic reaches the granted queue and
//! is WITHHELD from the one that grants nothing — and once that queue's policy is repaired,
//! the next transition reaches it too, while the notification withheld before the repair
//! never arrives. A client can therefore tell a missing grant apart from a system that
//! delivers nothing at all.
//!
//! The rows differ along what the rule generalises over: which metric the alarm watches (the
//! recorded ECS Container Insights one is one row), and, within each row, whether the
//! subscriber grants the send, and whether it granted it before or after the transition.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_on, received_messages, rpc, set_alarm_state,
    sns, subscribe_queue, Emulator,
};

/// `DescribeAlarms` for one alarm, as the state value it reports.
async fn alarm_state(emulator: &Emulator, name: &str) -> String {
    let described = rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every notification waiting on the queue at `url`, as the alarm documents SNS enveloped.
async fn notifications(emulator: &Emulator, url: &str) -> Vec<Value> {
    received_messages(emulator, url)
        .await
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            let envelope: Value = serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("the body is not the SNS envelope ({error}): {body}"));
            let inner = envelope["Message"].as_str().unwrap_or_default();
            serde_json::from_str(inner)
                .unwrap_or_else(|error| panic!("the message is not a notification ({error}): {inner}"))
        })
        .collect()
}

#[tokio::test]
async fn an_alarm_notification_is_withheld_from_a_queue_without_the_sns_grant() {
    // (case, the alarm's namespace, the metric it watches)
    let rows: &[(&str, &str, &str)] = &[
        (
            "an ECS Container Insights task count",
            "ECS/ContainerInsights",
            "RunningTaskCount",
        ),
        ("a customer metric", "Custom/Orders", "Backlog"),
    ];

    for (case, namespace, metric) in rows {
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "fan-out")]).await,
            "TopicArn",
        );
        let (granted_url, granted_arn) = given_queue(&emulator, "granted").await;
        grant_sns_delivery_on(&emulator, &granted_url, &granted_arn).await;
        let (withheld_url, withheld_arn) = given_queue(&emulator, "withheld").await;
        for endpoint in [&granted_arn, &withheld_arn] {
            subscribe_queue(&emulator, &topic, endpoint).await;
        }

        let alarm = format!("{metric}-alarm");
        rpc(
            &emulator,
            "monitoring",
            "GraniteServiceVersion20100801",
            "PutMetricAlarm",
            &json!({
                "AlarmName": alarm,
                "Namespace": namespace,
                "MetricName": metric,
                "Dimensions": [{"Name": "ClusterName", "Value": "obs-ecs-1"}],
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "LessThanThreshold",
                "AlarmActions": [topic],
            }),
        )
        .await;

        // The transition that fires the action.
        set_alarm_state(&emulator, &alarm, "ALARM").await;
        let reported = alarm_state(&emulator, &alarm).await;
        assert_eq!(
            reported, "ALARM",
            "case {case:?}: the alarm must report the state it was moved to, got \
             {reported:?}"
        );

        let delivered = notifications(&emulator, &granted_url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the granted queue must receive one notification, got {delivered:?}"
        );
        assert_eq!(
            delivered[0]["AlarmName"].as_str(),
            Some(alarm.as_str()),
            "case {case:?}: the notification must name the alarm, got {}",
            delivered[0]
        );
        assert_eq!(
            delivered[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the notification must carry the state moved to, got {}",
            delivered[0]
        );
        assert_eq!(
            delivered[0]["Trigger"]["MetricName"].as_str(),
            Some(*metric),
            "case {case:?}: the notification must name the metric evaluated, got {}",
            delivered[0]
        );
        let withheld = notifications(&emulator, &withheld_url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: the queue with no SNS grant must hold nothing, got {withheld:?}"
        );

        // The repair, and the next transition: the queue receives that one, and never the
        // notification withheld before it.
        grant_sns_delivery_on(&emulator, &withheld_url, &withheld_arn).await;
        set_alarm_state(&emulator, &alarm, "OK").await;
        set_alarm_state(&emulator, &alarm, "ALARM").await;
        let repaired = notifications(&emulator, &withheld_url).await;
        assert_eq!(
            repaired.len(),
            1,
            "case {case:?}: the repaired queue must receive the notification of the next \
             transition and only that one, got {repaired:?}"
        );
        assert_eq!(
            repaired[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the repaired queue must hold the ALARM notification — the OK \
             transition fires no action here, got {}",
            repaired[0]
        );
        let delivered = notifications(&emulator, &granted_url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the granted queue must receive the second ALARM notification too, \
             got {delivered:?}"
        );
    }
}
