// SPDX-License-Identifier: CC0-1.0

//! An alarm's SNS action is AUTHORIZED against the target topic's own access policy, end to
//! end over the wire through the real `MultiServiceRouter` with CloudWatch, SNS and SQS on
//! one engine — so what a client sees is what is asserted: the `Action` item the alarm's
//! history records, and the depth of the queue subscribed to the topic.
//!
//! The RULE asserted here: executing an alarm action is a cross-service request this service
//! makes as the service principal `cloudwatch.amazonaws.com`, so the target topic's own
//! resource policy decides it: an explicit deny of `SNS:Publish` for that principal on that
//! topic suppresses the publish — the topic's subscribers see nothing and the action is
//! recorded in the alarm's history with `actionState` `Failed`, an `error` naming the
//! authorization failure and the topic, and no `publishedMessage` — while a policy that
//! denies nothing about that publish leaves the action executed and `Succeeded`.
//!
//! The rows differ along what the rule generalises over: the FORM the topic's policy takes
//! (the one a topic with no policy of its own carries, an allow of this principal by name, an
//! allow of every principal, an allow of someone else, an allow of another action) and, for a
//! deny, WHICH request it is about — the principal it names, the action it names (by name or
//! as a wildcard), the resource it is scoped to and the condition it is scoped by.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_on, received_messages, rpc, set_alarm_state,
    sns, subscribe_queue, Emulator,
};

/// The service principal this emulator executes an alarm action as, which a topic policy
/// names under `Principal` as `{"Service": …}`.
pub(crate) const CLOUDWATCH_PRINCIPAL: &str = "cloudwatch.amazonaws.com";

/// How a refused publish names the action it was refused, as the recorded `error` carries it.
const REFUSED_ACTION: &str = "not authorized to perform: SNS:Publish";

/// The reading that refused the publish, as the recorded `error` words it.
const EXPLICIT_DENY: &str = "with an explicit deny in a resource-based policy";

/// One policy statement, as a topic's owner writes it: an effect, a principal, an action and
/// whatever else (`Resource`, `Condition`) the row scopes it by.
pub(crate) fn statement(effect: &str, principal: Value, action: Value, extra: Value) -> Value {
    let mut written = json!({
        "Sid": format!("{effect}Statement"),
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": "*",
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        written[member] = value;
    }
    written
}

/// A whole policy document carrying `statements`.
pub(crate) fn policy(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// An `Allow` of this publish for every principal: what a row that tests a DENY starts from,
/// so the deny is the only thing that can refuse it.
pub(crate) fn allow_everyone() -> Value {
    statement("Allow", json!("*"), json!("sns:Publish"), json!({}))
}

/// A topic called `name` with a queue subscribed to it: the topic's ARN and the queue's url.
///
/// The queue grants the SNS service principal the send, so a notification that IS published
/// reaches it and a queue that holds nothing holds nothing because of the TOPIC's policy. The
/// queue, the grant and the subscription are the harness's ([`crate::harness`]), shared with
/// every other suite that reads an alarm's notification off a queue.
pub(crate) async fn given_topic_with_subscriber(
    emulator: &Emulator,
    name: &str,
) -> (String, String) {
    let topic = element(
        &sns(emulator, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    let (url, queue_arn) = given_queue(emulator, &format!("{name}-subscriber")).await;
    grant_sns_delivery_on(emulator, &url, &queue_arn).await;
    subscribe_queue(emulator, &topic, &queue_arn).await;
    (topic, url)
}

/// Set the topic's access policy the way its owner does, and read it back: the decision is
/// taken on the policy the TOPIC reports, so a row that writes one asserts the topic holds it.
pub(crate) async fn set_topic_policy(emulator: &Emulator, topic_arn: &str, document: &str) {
    sns(
        emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn),
            ("AttributeName", "Policy"),
            ("AttributeValue", document),
        ],
    )
    .await;
    let read = sns(emulator, "GetTopicAttributes", &[("TopicArn", topic_arn)]).await;
    let reported = topic_policy_attribute(&read);
    let written: Value = serde_json::from_str(document).expect("the row writes a JSON policy");
    assert_eq!(
        reported, written,
        "the topic must hold the policy it was given, got {reported}"
    );
}

/// The `Policy` attribute out of an awsQuery `GetTopicAttributes` response, parsed.
///
/// The attributes come back as `<entry><key>…</key><value>…</value></entry>` pairs, and the
/// policy's own JSON arrives XML-escaped inside one of them.
fn topic_policy_attribute(body: &str) -> Value {
    let entry = body
        .split("<entry>")
        .find(|entry| entry.contains("<key>Policy</key>"))
        .unwrap_or_else(|| panic!("no Policy attribute in {body}"));
    let value = element(entry, "value");
    let unescaped = value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    serde_json::from_str(&unescaped)
        .unwrap_or_else(|error| panic!("the reported Policy is not JSON ({error}): {unescaped}"))
}

/// An alarm over a customer metric whose ALARM action is each of `topics`.
pub(crate) async fn given_alarm(emulator: &Emulator, name: &str, topics: &[&str]) {
    rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Custom/Orders",
            "MetricName": "Backlog",
            "Dimensions": [{"Name": "QueueName", "Value": name}],
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanOrEqualToThreshold",
            "AlarmActions": topics,
        }),
    )
    .await;
}

/// The ARN the alarm `name` reports for itself: the resource a cross-service publish it fires
/// comes FROM, which a topic policy may scope a statement by.
pub(crate) async fn alarm_arn(emulator: &Emulator, name: &str) -> String {
    let described = rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["AlarmArn"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeAlarms {name}: no AlarmArn in {described}"))
        .to_string()
}

/// Force the alarm `name` into `ALARM`: the transition that fires the actions.
async fn fire_alarm(emulator: &Emulator, name: &str) {
    set_alarm_state(emulator, name, "ALARM").await;
}

/// Every `Action` item the alarm's history holds, as the parsed `HistoryData` document of
/// each, beside the topic the item names.
pub(crate) async fn action_items(emulator: &Emulator, alarm: &str) -> Vec<(String, Value)> {
    let history = rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
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

/// How many notifications are waiting on the queue at `url`.
pub(crate) async fn delivered(emulator: &Emulator, url: &str) -> usize {
    received_messages(emulator, url).await.len()
}

/// How many notifications reach the queue at `url`, polled the way a client waiting for an
/// asynchronous delivery does: until `wanted` of them have arrived or the attempts run out.
///
/// A publish is asynchronous, so a count read once can observe a delivery still in flight;
/// what the rule decides is whether the notification arrives AT ALL, so the count is read
/// until it does.
pub(crate) async fn poll_delivered(emulator: &Emulator, url: &str, wanted: usize) -> usize {
    let mut arrived = 0;
    for _ in 0..20 {
        arrived += delivered(emulator, url).await;
        if arrived >= wanted {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    arrived
}

/// Assert one `Action` item against what the topic's policy said about the publish.
pub(crate) fn assert_action_item(case: &str, item: &Value, topic_arn: &str, published: bool) {
    let expected = if published { "Succeeded" } else { "Failed" };
    let state = item["actionState"].as_str().unwrap_or_default();
    assert_eq!(
        state, expected,
        "case {case:?}: expected actionState {expected:?}, got {item}"
    );
    let error = item["error"].as_str();
    let message = item["publishedMessage"].as_str();
    if published {
        assert_eq!(
            error, None,
            "case {case:?}: a published action records no error, got {item}"
        );
        // An executed action handed the notification over, so the record carries what was
        // published — the document naming the alarm that produced it.
        assert!(
            message.is_some_and(|published| published.contains("\"NewStateValue\"")),
            "case {case:?}: a published action must record the message it handed over, got \
             {item}"
        );
        return;
    }
    assert_eq!(
        message, None,
        "case {case:?}: a refused action hands nothing over, so it records no published \
         message, got {item}"
    );
    let error = error.unwrap_or_default();
    assert!(
        error.contains(REFUSED_ACTION) && error.contains(topic_arn) && error.contains(EXPLICIT_DENY),
        "case {case:?}: the error must name the {REFUSED_ACTION:?} failure, the topic \
         {topic_arn:?} and {EXPLICIT_DENY:?}, got {error:?}"
    );
}

/// The policy a row writes on its topic, built from the topic's ARN and the ARN of the alarm
/// that fires at it — so a row may scope a statement to either.
type PolicyFor = Box<dyn Fn(&str, &str) -> Option<String>>;

/// A row whose topic carries no policy of its own: the one the topic generates stands.
fn no_policy() -> PolicyFor {
    Box::new(|_topic, _alarm| None)
}

/// A row whose topic carries the statements `build` writes.
fn written(build: impl Fn(&str, &str) -> Vec<Value> + 'static) -> PolicyFor {
    Box::new(move |topic, alarm| Some(policy(build(topic, alarm))))
}

#[tokio::test]
async fn an_alarm_action_is_published_unless_the_topic_policy_denies_this_service() {
    // (case, the topic's policy, whether the notification is published)
    let rows: Vec<(&str, PolicyFor, bool)> = vec![
        (
            "the policy a topic with none of its own carries",
            no_policy(),
            true,
        ),
        (
            "an allow of this service principal by name",
            written(|_topic, _alarm| {
                vec![statement(
                    "Allow",
                    json!({"Service": CLOUDWATCH_PRINCIPAL}),
                    json!("sns:Publish"),
                    json!({}),
                )]
            }),
            true,
        ),
        (
            "an allow of every principal for every action of the service",
            written(|_topic, _alarm| {
                vec![statement("Allow", json!("*"), json!("sns:*"), json!({}))]
            }),
            true,
        ),
        (
            "an allow of another service principal only",
            written(|_topic, _alarm| {
                vec![statement(
                    "Allow",
                    json!({"Service": "events.amazonaws.com"}),
                    json!("sns:Publish"),
                    json!({}),
                )]
            }),
            true,
        ),
        (
            "an allow of this principal for another action only",
            written(|_topic, _alarm| {
                vec![statement(
                    "Allow",
                    json!({"Service": CLOUDWATCH_PRINCIPAL}),
                    json!("sns:Subscribe"),
                    json!({}),
                )]
            }),
            true,
        ),
        (
            "an explicit deny of this principal by name",
            written(|_topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:Publish"),
                        json!({}),
                    ),
                ]
            }),
            false,
        ),
        (
            "an explicit deny written as an action wildcard",
            written(|_topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:*"),
                        json!({}),
                    ),
                ]
            }),
            false,
        ),
        (
            "an explicit deny aimed at every principal",
            written(|_topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement("Deny", json!("*"), json!("sns:Publish"), json!({})),
                ]
            }),
            false,
        ),
        (
            "an explicit deny scoped to this very topic",
            written(|topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:Publish"),
                        json!({"Resource": topic}),
                    ),
                ]
            }),
            false,
        ),
        (
            "an explicit deny scoped to the alarm this publish comes from",
            written(|_topic, alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:Publish"),
                        json!({"Condition": {"ArnEquals": {"aws:SourceArn": alarm}}}),
                    ),
                ]
            }),
            false,
        ),
        (
            "an explicit deny of another service principal",
            written(|_topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": "events.amazonaws.com"}),
                        json!("sns:Publish"),
                        json!({}),
                    ),
                ]
            }),
            true,
        ),
        (
            "an explicit deny of another action for this principal",
            written(|_topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:Subscribe"),
                        json!({}),
                    ),
                ]
            }),
            true,
        ),
        (
            "an explicit deny scoped to another topic",
            written(|topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:Publish"),
                        json!({"Resource": format!("{topic}-elsewhere")}),
                    ),
                ]
            }),
            true,
        ),
        (
            "an explicit deny scoped to an account this publish does not come from",
            written(|_topic, _alarm| {
                vec![
                    allow_everyone(),
                    statement(
                        "Deny",
                        json!({"Service": CLOUDWATCH_PRINCIPAL}),
                        json!("sns:Publish"),
                        json!({
                            "Condition": {
                                "StringEquals": {"aws:SourceAccount": "999999999999"}
                            }
                        }),
                    ),
                ]
            }),
            true,
        ),
    ];

    for (case, document, published) in rows {
        let emulator = emulator();
        let (topic, queue) = given_topic_with_subscriber(&emulator, "orders").await;
        let alarm = "orders-backlog-high";
        given_alarm(&emulator, alarm, &[&topic]).await;
        let arn = alarm_arn(&emulator, alarm).await;
        if let Some(written) = document(&topic, &arn) {
            set_topic_policy(&emulator, &topic, &written).await;
        }
        fire_alarm(&emulator, alarm).await;

        let items = action_items(&emulator, alarm).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the one action executed must record one Action item, got {items:?}"
        );
        let (resource, item) = &items[0];
        assert_eq!(
            resource, &topic,
            "case {case:?}: the item must name the topic the action aimed at, got {item}"
        );
        assert_action_item(case, item, &topic, published);

        let waiting = poll_delivered(&emulator, &queue, usize::from(published)).await;
        assert_eq!(
            waiting,
            usize::from(published),
            "case {case:?}: expected {} notification(s) on the subscribed queue, got {waiting}",
            usize::from(published)
        );
    }
}

#[tokio::test]
async fn one_transition_decides_each_of_its_actions_on_its_own_topic() {
    // Two topics fired by ONE transition: the decision is taken per topic, so the one whose
    // policy denies nothing is published and delivered while the denying one is suppressed —
    // and a client can tell the two apart in the same alarm's history.
    let emulator = emulator();
    let (allowed, allowed_queue) = given_topic_with_subscriber(&emulator, "allowed").await;
    let (denied, denied_queue) = given_topic_with_subscriber(&emulator, "denied").await;
    set_topic_policy(
        &emulator,
        &denied,
        &policy(vec![
            allow_everyone(),
            statement(
                "Deny",
                json!({"Service": CLOUDWATCH_PRINCIPAL}),
                json!("sns:Publish"),
                json!({"Resource": denied.clone()}),
            ),
        ]),
    )
    .await;

    let alarm = "orders-backlog-high";
    given_alarm(&emulator, alarm, &[&allowed, &denied]).await;
    fire_alarm(&emulator, alarm).await;

    let items = action_items(&emulator, alarm).await;
    assert_eq!(
        items.len(),
        2,
        "both actions of the transition must be recorded, got {items:?}"
    );
    for (case, topic, published) in [
        ("the topic that denies nothing", &allowed, true),
        ("the denying topic", &denied, false),
    ] {
        let (_, item) = items
            .iter()
            .find(|(resource, _)| resource == topic)
            .unwrap_or_else(|| panic!("no Action item for {topic}: {items:?}"));
        assert_action_item(case, item, topic, published);
    }

    let reached = poll_delivered(&emulator, &allowed_queue, 1).await;
    assert_eq!(
        reached, 1,
        "the topic that denies nothing must deliver its notification, got {reached}"
    );
    let withheld = delivered(&emulator, &denied_queue).await;
    assert_eq!(
        withheld, 0,
        "the denying topic's subscriber must receive nothing, got {withheld}"
    );
}

#[tokio::test]
async fn a_topic_whose_deny_is_removed_publishes_the_next_transition() {
    // The deny is the ONLY thing suppressing the publish: once it is gone, the next
    // transition's action to the same topic succeeds and is delivered, and the notification
    // suppressed before the repair never arrives.
    let emulator = emulator();
    let (topic, queue) = given_topic_with_subscriber(&emulator, "orders").await;
    set_topic_policy(
        &emulator,
        &topic,
        &policy(vec![
            allow_everyone(),
            statement(
                "Deny",
                json!({"Service": CLOUDWATCH_PRINCIPAL}),
                json!("sns:Publish"),
                json!({}),
            ),
        ]),
    )
    .await;
    let alarm = "orders-backlog-high";
    given_alarm(&emulator, alarm, &[&topic]).await;
    fire_alarm(&emulator, alarm).await;
    let withheld = delivered(&emulator, &queue).await;
    assert_eq!(
        withheld, 0,
        "the denied publish must deliver nothing, got {withheld}"
    );

    // The removal, and the next transition. The alarm has no OK action, so returning it to
    // OK fires nothing and only the second ALARM transition publishes.
    set_topic_policy(&emulator, &topic, &policy(vec![allow_everyone()])).await;
    set_alarm_state(&emulator, alarm, "OK").await;
    fire_alarm(&emulator, alarm).await;

    let reached = poll_delivered(&emulator, &queue, 1).await;
    assert_eq!(
        reached, 1,
        "the repaired topic must receive the next transition's notification and only that \
         one, got {reached}"
    );
    let items = action_items(&emulator, alarm).await;
    assert_eq!(
        items.len(),
        2,
        "each executed action must be recorded, got {items:?}"
    );
    let states: Vec<&str> = items
        .iter()
        .map(|(_, item)| item["actionState"].as_str().unwrap_or_default())
        .collect();
    assert!(
        states.contains(&"Failed") && states.contains(&"Succeeded"),
        "the history must hold the refused action and the published one, got {items:?}"
    );
}
