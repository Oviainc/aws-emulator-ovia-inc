// SPDX-License-Identifier: CC0-1.0

//! What a client that waits on the ACTION'S TARGET sees, end to end over the wire through the
//! real `MultiServiceRouter` with CloudWatch, SNS and SQS on one engine.
//!
//! The RULE asserted here: **an alarm whose metric holds no datapoint at all and whose
//! `TreatMissingData` is `breaching` is moved to `ALARM` by the evaluation the clock owes —
//! nothing forces its state and nothing calls this service again — and each `AlarmActions`
//! entry that transition fires is executed against the target topic's own access policy, so
//! the notification reaches that topic's subscriber and the alarm's history records one
//! `Action` item per action.** The second half of that rule is what a client depends on and
//! what no other suite here asserts: between the write that configures the alarm and the
//! notification arriving on the queue, NO CloudWatch call is made at all, so the delivery is
//! the clock's own and not some later read's.
//!
//! The rows of the first table differ along what the rule generalises over — the namespace the
//! absent series lives in (an AWS namespace and a customer one), the statistic its windows
//! would be aggregated with, how many periods are evaluated, how many of them must breach, the
//! period's resolution, and whether the alarm watches one `Unit` — because the rule is about a
//! series with NO datapoints, which every one of those reads the same way.
//!
//! The second test is the two-topic shape a client writes when it uses a topic policy to stop
//! one target: the policy each topic REPORTS is the one the decision is taken on, so the deny
//! is appended to the document the topic itself carries, as its owner would, and the repair is
//! the removal of that one statement.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, received_messages, rpc, sns, subscribe_queue, Emulator,
};

/// The service principal this emulator executes an alarm action as, which a topic policy names
/// under `Principal` as `{"Service": …}`.
const CLOUDWATCH_PRINCIPAL: &str = "cloudwatch.amazonaws.com";

/// How a refused publish names the action it was refused, as the recorded `error` carries it.
const REFUSED_ACTION: &str = "not authorized to perform: SNS:Publish";

/// How long a row waits for an asynchronous delivery, and how often it looks. Generous only so
/// a failure is a failure rather than a hang: the transition is owed inside the framework's
/// 250-750 ms window and the delivery follows it.
const DELIVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(100);

/// One CloudWatch call, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        operation,
        body,
    )
    .await
}

/// A topic called `name`, with the queue at `queue_arn` subscribed to it.
async fn given_topic(emulator: &Emulator, name: &str, queue_arn: &str) -> String {
    let topic = element(
        &sns(emulator, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    subscribe_queue(emulator, &topic, queue_arn).await;
    topic
}

/// Grant the SNS service principal the send on the queue at `url`, scoped by `aws:SourceArn`
/// to `topics` — the grant AWS documents for a queue subscribed to those topics, written the
/// way a client that subscribes one queue to SEVERAL topics writes it.
async fn grant_sns_delivery_from(emulator: &Emulator, url: &str, queue_arn: &str, topics: &[&str]) {
    rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowSNSDelivery",
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue_arn,
                    "Condition": {"ArnEquals": {"aws:SourceArn": topics}},
                }],
            })
            .to_string()},
        }),
    )
    .await;
}

/// The `Policy` attribute the topic `topic_arn` REPORTS, parsed: the document the publish
/// decision is taken on, including the one a topic with no policy of its own carries.
///
/// The attributes come back as `<entry><key>…</key><value>…</value></entry>` pairs, and the
/// policy's own JSON arrives XML-escaped inside one of them.
async fn reported_topic_policy(emulator: &Emulator, topic_arn: &str) -> Value {
    let read = sns(emulator, "GetTopicAttributes", &[("TopicArn", topic_arn)]).await;
    let entry = read
        .split("<entry>")
        .find(|entry| entry.contains("<key>Policy</key>"))
        .unwrap_or_else(|| panic!("no Policy attribute in {read}"));
    let unescaped = element(entry, "value")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    serde_json::from_str(&unescaped)
        .unwrap_or_else(|error| panic!("the reported Policy is not JSON ({error}): {unescaped}"))
}

/// Write `document` as the topic's access policy, the way its owner does, and read it back:
/// the decision is taken on the policy the TOPIC reports.
async fn set_topic_policy(emulator: &Emulator, topic_arn: &str, document: &Value) {
    sns(
        emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn),
            ("AttributeName", "Policy"),
            ("AttributeValue", &document.to_string()),
        ],
    )
    .await;
    let reported = reported_topic_policy(emulator, topic_arn).await;
    assert_eq!(
        &reported, document,
        "the topic must hold the policy it was given, got {reported}"
    );
}

/// The topic's own policy with one statement appended, as a client that wants to stop ONE
/// principal writes it: whatever the topic already carries, plus the statement.
async fn with_appended_statement(emulator: &Emulator, topic_arn: &str, statement: Value) -> Value {
    let mut document = reported_topic_policy(emulator, topic_arn).await;
    let Some(statements) = document["Statement"].as_array_mut() else {
        panic!("the reported Policy states no Statement array: {document}");
    };
    statements.push(statement);
    document
}

/// An explicit deny of this service's publish on `topic_arn`, written the way an owner that
/// wants CloudWatch kept out of its topic writes it.
fn deny_cloudwatch_publish(topic_arn: &str) -> Value {
    json!({
        "Sid": "DenyCloudWatchPublish",
        "Effect": "Deny",
        "Principal": {"Service": CLOUDWATCH_PRINCIPAL},
        "Action": "sns:Publish",
        "Resource": topic_arn,
    })
}

/// One alarm over a metric NOTHING has published a datapoint for, whose missing data is
/// breaching: nothing forces its state, so only the evaluation the clock owes can move it.
async fn given_breaching_alarm(emulator: &Emulator, name: &str, configuration: &Value) {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": "AWS/RDS",
        "MetricName": "DatabaseConnections",
        "Dimensions": [{"Name": "DBInstanceIdentifier", "Value": format!("{name}-instance")}],
        "Statistic": "Maximum",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanOrEqualToThreshold",
        "TreatMissingData": "breaching",
    });
    for (member, value) in configuration.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            // A row that states a member as null is a row that leaves it OUT: `Statistic` and
            // `ExtendedStatistic` are alternatives, so an alarm carries one of the two.
            if let Some(members) = request.as_object_mut() {
                members.remove(&member);
            }
            continue;
        }
        request[member] = value;
    }
    cw(emulator, "PutMetricAlarm", &request).await;
}

/// Every message waiting on the queue at `url`, as the SNS envelope each body parses to.
async fn envelopes(emulator: &Emulator, url: &str) -> Vec<Value> {
    received_messages(emulator, url)
        .await
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("a delivery is not the SNS envelope ({error}): {body}")
            })
        })
        .collect()
}

/// Wait — reading NOTHING but the queue — until `wanted` notifications have arrived on it or
/// the deadline passes, and answer every envelope that did arrive.
///
/// The one thing this suite is about: no CloudWatch call is made while waiting, so what
/// arrives is what the alarm clock delivered on its own.
async fn delivered_without_calling_cloudwatch(
    emulator: &Emulator,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        arrived.extend(envelopes(emulator, url).await);
        if arrived.len() >= wanted || std::time::Instant::now() >= deadline {
            return arrived;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// Wait — reading NOTHING but the queue — for `span`, and answer every envelope that arrived
/// in it: how a client establishes that a notification does NOT come.
async fn arrivals_over(
    emulator: &Emulator,
    url: &str,
    span: std::time::Duration,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + span;
    let mut arrived: Vec<Value> = Vec::new();
    while std::time::Instant::now() < deadline {
        arrived.extend(envelopes(emulator, url).await);
        std::thread::sleep(LOOK_AGAIN);
    }
    arrived
}

/// The `Action` items the alarm's history holds, as the topic each names beside its parsed
/// `HistoryData`.
async fn action_items(emulator: &Emulator, alarm: &str) -> Vec<(String, Value)> {
    let history = cw(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": alarm,
            "HistoryItemType": "Action",
            "AlarmTypes": ["MetricAlarm"],
        }),
    )
    .await;
    history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let data = item["HistoryData"].as_str().unwrap_or_default();
            let parsed: Value = serde_json::from_str(data)
                .unwrap_or_else(|error| panic!("HistoryData is not JSON ({error}): {data}"));
            let resource = parsed["notificationResource"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (resource, parsed)
        })
        .collect()
}

/// Poll the alarm's `Action` history until it holds `wanted` items or the deadline passes.
async fn polled_action_items(
    emulator: &Emulator,
    alarm: &str,
    wanted: usize,
) -> Vec<(String, Value)> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        let items = action_items(emulator, alarm).await;
        if items.len() >= wanted || std::time::Instant::now() >= deadline {
            return items;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// The runtime flavour the booted emulator serves on, so the clock runs the way it does in
/// production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_breaching_alarm_delivers_through_the_clock_with_no_further_cloudwatch_call() {
    // (case, what the alarm carries beyond the breaching configuration above)
    let rows: Vec<(&str, Value)> = vec![
        ("the configuration as recorded", json!({})),
        (
            "a series in a customer namespace",
            json!({"Namespace": "Custom/Connections", "MetricName": "Backlog"}),
        ),
        (
            "another AWS namespace and statistic",
            json!({
                "Namespace": "AWS/ECS",
                "MetricName": "RunningTaskCount",
                "Statistic": "Average",
            }),
        ),
        (
            "a percentile statistic",
            json!({"Statistic": null, "ExtendedStatistic": "p99"}),
        ),
        (
            "three periods, every one of them missing",
            json!({"EvaluationPeriods": 3}),
        ),
        (
            "one of three periods must breach",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 1}),
        ),
        ("a high-resolution period", json!({"Period": 10})),
        ("an alarm that watches one unit", json!({"Unit": "Count"})),
    ];

    for (case, configuration) in rows {
        let emulator = emulator();
        let (url, queue_arn) = given_queue(&emulator, "subscriber").await;
        let topic = given_topic(&emulator, "notifications", &queue_arn).await;
        grant_sns_delivery_from(&emulator, &url, &queue_arn, &[&topic]).await;

        let alarm = "connections-high";
        let mut request = configuration.clone();
        request["AlarmActions"] = json!([topic]);
        given_breaching_alarm(&emulator, alarm, &request).await;

        // From here until the notification arrives, the only service called is SQS.
        let arrived = delivered_without_calling_cloudwatch(&emulator, &url, 1).await;
        assert_eq!(
            arrived.len(),
            1,
            "case {case:?}: the clock's transition must deliver exactly one notification with \
             no further CloudWatch call, got {arrived:?}"
        );
        let envelope = &arrived[0];
        assert_eq!(
            envelope["TopicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the delivery must name the topic it came from, got {envelope}"
        );
        let message: Value = serde_json::from_str(envelope["Message"].as_str().unwrap_or_default())
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the published Message is not JSON ({error}): {envelope}")
            });
        assert_eq!(
            message["AlarmName"].as_str(),
            Some(alarm),
            "case {case:?}: the notification must name the alarm, got {message}"
        );
        assert_eq!(
            message["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: a breaching evaluation must report ALARM, got {message}"
        );
        let watched = request["Namespace"].as_str().unwrap_or("AWS/RDS").to_string();
        assert_eq!(
            message["Trigger"]["Namespace"].as_str(),
            Some(watched.as_str()),
            "case {case:?}: the Trigger must report the series the alarm evaluated, got {message}"
        );

        // And the same transition is in the alarm's own history, as one executed action.
        let items = polled_action_items(&emulator, alarm, 1).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the executed action must be recorded once, got {items:?}"
        );
        let (resource, item) = &items[0];
        assert_eq!(
            resource, &topic,
            "case {case:?}: the item must name the action's target, got {item}"
        );
        assert_eq!(
            item["actionState"].as_str(),
            Some("Succeeded"),
            "case {case:?}: an action the target accepted is Succeeded, got {item}"
        );
        // The state the evaluation reached is the state a client reads back, too.
        let described = cw(&emulator, "DescribeAlarms", &json!({"AlarmNames": [alarm]})).await;
        assert_eq!(
            described["MetricAlarms"][0]["StateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the alarm must report the state it moved to, got {described}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_policy_each_topic_reports_decides_that_topics_action_and_a_repair_is_delivered() {
    // Two topics subscribed to one queue, one of them carrying the policy it was created with
    // plus an explicit deny of this service's publish — the shape a client writes when it uses
    // a topic policy to stop one target — and ONE alarm firing both.
    let emulator = emulator();
    let (url, queue_arn) = given_queue(&emulator, "subscriber").await;
    let allowed = given_topic(&emulator, "allowed", &queue_arn).await;
    let denied = given_topic(&emulator, "denied", &queue_arn).await;
    grant_sns_delivery_from(&emulator, &url, &queue_arn, &[&allowed, &denied]).await;
    let repaired = reported_topic_policy(&emulator, &denied).await;
    let with_deny =
        with_appended_statement(&emulator, &denied, deny_cloudwatch_publish(&denied)).await;
    set_topic_policy(&emulator, &denied, &with_deny).await;

    let alarm = "connections-high";
    given_breaching_alarm(
        &emulator,
        alarm,
        &json!({"AlarmActions": [allowed.clone(), denied.clone()]}),
    )
    .await;

    let items = polled_action_items(&emulator, alarm, 2).await;
    assert_eq!(
        items.len(),
        2,
        "both actions of the transition must be recorded, got {items:?}"
    );
    for (case, topic, published) in [
        (
            "the topic whose policy says nothing about this publish",
            &allowed,
            true,
        ),
        ("the topic whose policy denies it", &denied, false),
    ] {
        let (_, item) = items
            .iter()
            .find(|(resource, _)| resource == topic)
            .unwrap_or_else(|| panic!("no Action item for {topic}: {items:?}"));
        let state = item["actionState"].as_str().unwrap_or_default();
        let expected = if published { "Succeeded" } else { "Failed" };
        assert_eq!(
            state, expected,
            "case {case:?}: expected actionState {expected:?}, got {item}"
        );
        if published {
            assert!(
                item["publishedMessage"]
                    .as_str()
                    .is_some_and(|published| published.contains("\"NewStateValue\"")),
                "case {case:?}: an executed action records what it handed over, got {item}"
            );
            continue;
        }
        let error = item["error"].as_str().unwrap_or_default();
        assert!(
            error.contains(REFUSED_ACTION) && error.contains(topic.as_str()),
            "case {case:?}: the error must name the {REFUSED_ACTION:?} failure and the topic \
             {topic:?}, got {error:?}"
        );
        assert_eq!(
            item["publishedMessage"].as_str(),
            None,
            "case {case:?}: a refused action hands nothing over, so it records no published \
             message, got {item}"
        );
    }

    // One notification on the queue, from the topic that was not denied — and nothing from the
    // denied one, however much longer a client waits for it.
    let mut arrived = delivered_without_calling_cloudwatch(&emulator, &url, 1).await;
    arrived.extend(arrivals_over(&emulator, &url, std::time::Duration::from_secs(1)).await);
    let sources: Vec<&str> = arrived
        .iter()
        .map(|envelope| envelope["TopicArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        sources,
        vec![allowed.as_str()],
        "only the topic that was not denied may deliver, got {arrived:?}"
    );

    // The deny was the ONLY thing suppressing it: with the statement removed, a fresh alarm's
    // action to that same topic is executed and delivered.
    set_topic_policy(&emulator, &denied, &repaired).await;
    let fresh = "connections-high-again";
    given_breaching_alarm(&emulator, fresh, &json!({"AlarmActions": [denied.clone()]})).await;
    let arrived = delivered_without_calling_cloudwatch(&emulator, &url, 1).await;
    let sources: Vec<&str> = arrived
        .iter()
        .map(|envelope| envelope["TopicArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        sources,
        vec![denied.as_str()],
        "the repaired topic must deliver the fresh alarm's notification, got {arrived:?}"
    );
    let items = polled_action_items(&emulator, fresh, 1).await;
    assert_eq!(
        items.len(),
        1,
        "the fresh alarm's action must be recorded, got {items:?}"
    );
    assert_eq!(
        items[0].1["actionState"].as_str(),
        Some("Succeeded"),
        "nothing denies the publish any more, so the action succeeds, got {:?}",
        items[0].1
    );
}
