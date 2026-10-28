// SPDX-License-Identifier: CC0-1.0

//! A metric alarm nothing publishes a datapoint for fires its actions on the EMULATOR'S OWN
//! clock — through the whole mounted service set, the one `core/router/src/lib.rs` assembles
//! for the binary.
//!
//! The RULE under test: **an alarm whose series holds no datapoint at all and whose
//! `TreatMissingData` is `breaching` leaves `INSUFFICIENT_DATA` for `ALARM` on the evaluation
//! the clock owes — no client call forces it — and each `AlarmActions` entry that transition
//! fires is executed against the target topic's OWN access policy: an entry no statement
//! denies is published, so the queue subscribed to that topic receives the notification, and
//! an entry whose topic explicitly denies this service the publish is withheld; either way the
//! alarm's history records one `Action` item per entry, naming the target, whether it
//! succeeded, what was published when it did and the authorization failure when it did not.**
//!
//! Four services have to agree for that to hold — RDS for the identifier the watched dimension
//! names (and does not hold), CloudWatch for the evaluation and the publish, SNS for the
//! policy decision and the fan-out, SQS for what arrived — and the publish CloudWatch makes is
//! an in-process call addressed the way a client's own `sns`-scoped call is, so WHICH mounted
//! model claims it is only decidable with every service mounted. The crate-local suites
//! (`services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/tests/all/`) mount
//! the three services they need and pin the evaluation, the policy decision and the delivery
//! over tables of alarms, policies and grants; what only this suite can reach is that same
//! clock-driven path over the production service set — the sibling router suite
//! (`cloudwatch_alarm_action_reaches_its_subscribed_queue.rs`) drives transitions a client
//! FORCES with `SetAlarmState`, so no suite here covered the one the clock makes due.
//!
//! The rows differ along what the rule generalises over — the Region the whole wiring lives
//! in, the namespace the absent series lives in (an AWS one and a customer one), the statistic
//! its windows would be aggregated with, how many periods are evaluated and how many of them
//! must breach, the period's resolution, and the direction of the comparison — because the
//! rule is about a series with NO datapoints, which every one of those reads the same way.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};
// The stack the router assembly needs and the awsQuery API version the RDS front door
// declares come from the suite that owns them rather than a second copy of each.
use crate::rds_docdb_neptune_signing_name_routing::{API_VERSION as RDS_API_VERSION, BOOT_STACK_SIZE};

/// CloudWatch's `X-Amz-Target` prefix and its signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The awsQuery API version Amazon SNS declares.
const SNS_API_VERSION: &str = "2010-03-31";

/// The service principal CloudWatch executes an alarm action as, which a topic policy names
/// under `Principal` as `{"Service": …}`.
const CLOUDWATCH_PRINCIPAL: &str = "cloudwatch.amazonaws.com";

/// The service principal SNS performs its delivery to a subscribed queue as, which is what
/// that queue's own policy has to allow.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// How a refused publish names the action it was refused, as the recorded `error` carries it.
const REFUSED_PUBLISH: &str = "not authorized to perform: SNS:Publish";

/// The dimension the watched series is identified by, and the RDS member that names the same
/// resource — the identifier the alarm's dimension names must name no DB instance.
const DB_INSTANCE_DIMENSION: &str = "DBInstanceIdentifier";

/// The `awsQueryError` code Amazon RDS answers for an identifier it holds no instance for.
const DB_INSTANCE_NOT_FOUND: &str = "DBInstanceNotFound";

/// How long a row waits for an asynchronous delivery, and how often it looks. Generous only so
/// a failure is a failure rather than a hang: the transition is owed inside the framework's
/// window for a transition the clock makes due, and the delivery follows it.
const DELIVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a row watches a settled alarm for a delivery it must NOT make again: long enough
/// to span several of the clock's own wake-ups, so a re-publish on every later evaluation of
/// the same (unchanged) state would show up here.
const SETTLED_WATCH: std::time::Duration = std::time::Duration::from_secs(2);

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service` in `region`, which is what
/// routes the request. The signature is never checked: the router above runs with request
/// authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One `X-Amz-Target` RPC call against the service whose signing name is `signing_name`,
/// which must answer 200.
async fn rpc(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("authorization", authorization(signing_name, region))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One CloudWatch call (`awsJson1_0`).
async fn cw(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    rpc(
        router,
        case,
        region,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
async fn sqs(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    rpc(router, case, region, "sqs", "AmazonSQS", operation, body).await
}

/// One awsQuery form call, with the status and body it answered.
async fn form_call(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("authorization", authorization(signing_name, region))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Amazon SNS call over the awsQuery form wire that service serves, which must answer 200.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, text) = form_call(router, region, "sns", SNS_API_VERSION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {text}"
    );
    text
}

/// The precondition the watched series rests on: Amazon RDS holds NO DB instance under
/// `identifier`, so nothing publishes a datapoint for the dimension the alarm names.
async fn assert_no_such_db_instance(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    identifier: &str,
) {
    let (status, text) = form_call(
        router,
        region,
        "rds",
        RDS_API_VERSION,
        "DescribeDBInstances",
        &[(DB_INSTANCE_DIMENSION, identifier)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: a read of the identifier the alarm watches must be a 404, got {status} \
         with {text}"
    );
    let code = element(&text, "Code");
    assert_eq!(
        code, DB_INSTANCE_NOT_FOUND,
        "case {case:?}: expected {DB_INSTANCE_NOT_FOUND}, got {code:?} in {text}"
    );
}

/// A queue called `name`: its url and its ARN.
async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> (String, String) {
    let created = sqs(router, case, region, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        region,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// A topic called `name`, with the queue at `queue_arn` subscribed to it.
async fn given_topic(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    queue_arn: &str,
) -> String {
    let topic = element(
        &sns(router, case, region, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    sns(
        router,
        case,
        region,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    topic
}

/// Grant the SNS service principal the send on the queue at `url`, scoped by `aws:SourceArn`
/// to `topics` — the grant AWS documents for a queue subscribed to those topics, written the
/// way a client that subscribes one queue to SEVERAL topics writes it.
async fn grant_sns_delivery_from(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    queue_arn: &str,
    topics: &[&str],
) {
    sqs(
        router,
        case,
        region,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowSNSDelivery",
                    "Effect": "Allow",
                    "Principal": {"Service": SNS_PRINCIPAL},
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

/// The `Policy` attribute the topic at `topic_arn` REPORTS, parsed: the document the publish
/// decision is taken on, including the one a topic with no policy of its own carries.
async fn reported_topic_policy(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    topic_arn: &str,
) -> Value {
    let read = sns(
        router,
        case,
        region,
        "GetTopicAttributes",
        &[("TopicArn", topic_arn)],
    )
    .await;
    let entry = read
        .split("<entry>")
        .find(|entry| entry.contains("<key>Policy</key>"))
        .unwrap_or_else(|| panic!("case {case:?}: no Policy attribute in {read}"));
    let document = elements(entry, "value")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the Policy entry carries no value: {entry}"));
    serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("case {case:?}: the reported Policy is not JSON ({error}): {document}")
    })
}

/// Write `document` as the topic's access policy, the way its owner does, and read it back:
/// the decision is taken on the policy the TOPIC reports.
async fn set_topic_policy(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    topic_arn: &str,
    document: &Value,
) {
    sns(
        router,
        case,
        region,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn),
            ("AttributeName", "Policy"),
            ("AttributeValue", &document.to_string()),
        ],
    )
    .await;
    let reported = reported_topic_policy(router, case, region, topic_arn).await;
    assert_eq!(
        &reported, document,
        "case {case:?}: the topic must hold the policy it was given, got {reported}"
    );
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

/// `request` with `patch` merged in: every member the patch names replaces the one the request
/// carried, and a member the patch names `null` is REMOVED — an alarm carries `Statistic` or
/// `ExtendedStatistic`, never both, so a row that names one has to drop the other.
fn merged(mut request: Value, patch: &Value) -> Value {
    let members = request
        .as_object_mut()
        .expect("the request being patched is a JSON object");
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            members.remove(&member);
        } else {
            members.insert(member, value);
        }
    }
    request
}

/// The alarm this suite drives: one metric alarm over a series NOTHING publishes a datapoint
/// for, whose missing data is breaching, aimed at `topics`.
fn breaching_alarm(name: &str, instance: &str, topics: &[&str]) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": "AWS/RDS",
        "MetricName": "DatabaseConnections",
        "Dimensions": [{"Name": DB_INSTANCE_DIMENSION, "Value": instance}],
        "Statistic": "Maximum",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanOrEqualToThreshold",
        "TreatMissingData": "breaching",
        "AlarmActions": topics,
    })
}

/// Every message waiting on the queue at `url`, as the SNS envelope each body parses to.
async fn envelopes(router: &MultiServiceRouter, case: &str, region: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        region,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivery is not the SNS envelope ({error}): {body}")
            })
        })
        .collect()
}

/// Wait — reading NOTHING but the queue — until `wanted` notifications have arrived on it or
/// the deadline passes, and answer every envelope that did arrive.
///
/// The one thing this half of the rule is about: no CloudWatch call is made while waiting, so
/// what arrives is what the alarm clock delivered on its own.
async fn delivered_without_calling_cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        arrived.extend(envelopes(router, case, region, url).await);
        if arrived.len() >= wanted || std::time::Instant::now() >= deadline {
            return arrived;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// Wait for `span`, reading nothing but the queue, and answer every envelope that arrived in
/// it: how a client establishes that a notification does NOT come.
async fn arrivals_over(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    span: std::time::Duration,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + span;
    let mut arrived: Vec<Value> = Vec::new();
    while std::time::Instant::now() < deadline {
        arrived.extend(envelopes(router, case, region, url).await);
        std::thread::sleep(LOOK_AGAIN);
    }
    arrived
}

/// The history items of `item_type` the alarm `alarm` holds, as `DescribeAlarmHistory` reports
/// them to a client asking for metric alarms.
async fn history_items(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    item_type: &str,
) -> Vec<Value> {
    let history = cw(
        router,
        case,
        region,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": alarm,
            "HistoryItemType": item_type,
            "AlarmTypes": ["MetricAlarm"],
        }),
    )
    .await;
    history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `Action` items the alarm's history holds, as the target each names beside its parsed
/// `HistoryData`.
async fn action_items(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
) -> Vec<(String, Value)> {
    history_items(router, case, region, alarm, "Action")
        .await
        .iter()
        .map(|item| {
            let data = item["HistoryData"].as_str().unwrap_or_default();
            let parsed: Value = serde_json::from_str(data).unwrap_or_else(|error| {
                panic!("case {case:?}: HistoryData is not JSON ({error}): {data}")
            });
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
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    wanted: usize,
) -> Vec<(String, Value)> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        let items = action_items(router, case, region, alarm).await;
        if items.len() >= wanted || std::time::Instant::now() >= deadline {
            return items;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// What one `Action` item must report: the target it names, and whether the publish happened.
fn assert_action_item(case: &str, item: &Value, topic: &str, published: bool) {
    let resource = item["notificationResource"].as_str().unwrap_or_default();
    assert_eq!(
        resource, topic,
        "case {case:?}: the item must name the action's target, got {item}"
    );
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
                .is_some_and(|message| message.contains("\"NewStateValue\"")),
            "case {case:?}: an executed action records what it handed over, got {item}"
        );
        return;
    }
    let error = item["error"].as_str().unwrap_or_default();
    assert!(
        error.contains(REFUSED_PUBLISH) && error.contains(topic),
        "case {case:?}: the error must name the {REFUSED_PUBLISH:?} failure and the topic \
         {topic:?}, got {error:?}"
    );
    assert_eq!(
        item["publishedMessage"].as_str(),
        None,
        "case {case:?}: a refused action hands nothing over, so it records no published \
         message, got {item}"
    );
}

/// The alarm's state as `DescribeAlarms` reports it.
async fn reported_state(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
) -> String {
    let described = cw(router, case, region, "DescribeAlarms", &json!({"AlarmNames": [alarm]})).await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assembling the router costs stack (see `boot_serve.rs`), and the clock this suite is about
/// runs on its OWN task — so the suite is driven on an explicitly sized thread, on a
/// multi-thread runtime whose workers get the same stack, where that task really runs while
/// this one waits.
#[test]
fn a_breaching_alarm_fires_each_action_against_its_targets_policy_on_the_clock() {
    let thread = std::thread::Builder::new()
        .name("cloudwatch-breaching-alarm".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(BOOT_STACK_SIZE)
                .enable_all()
                .build()
                .expect("a multi-thread runtime");
            runtime.block_on(breaching_alarm_rows());
        })
        .expect("the suite's thread starts");
    if let Err(panic) = thread.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn breaching_alarm_rows() {
    // (case, the Region the whole wiring lives in, what the alarm carries beyond the
    // configuration above)
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the configuration as recorded", "us-west-2", json!({})),
        (
            "a customer namespace, a percentile statistic and three evaluated periods in \
             another Region",
            "eu-west-1",
            json!({
                "Namespace": "Custom/Connections",
                "MetricName": "Backlog",
                "Statistic": null,
                "ExtendedStatistic": "p90",
                "EvaluationPeriods": 3,
            }),
        ),
        (
            "a high-resolution period, two of three datapoints and the opposite comparison",
            "us-east-1",
            json!({
                "Statistic": "Minimum",
                "Period": 10,
                "EvaluationPeriods": 3,
                "DatapointsToAlarm": 2,
                "ComparisonOperator": "LessThanThreshold",
            }),
        ),
    ];

    for (case, region, patch) in rows {
        let router = router();
        let (url, queue_arn) = given_queue(&router, case, region, "subscriber").await;
        let allowed = given_topic(&router, case, region, "allowed", &queue_arn).await;
        let denied = given_topic(&router, case, region, "denied", &queue_arn).await;
        grant_sns_delivery_from(
            &router,
            case,
            region,
            &url,
            &queue_arn,
            &[&allowed, &denied],
        )
        .await;

        // The policy each topic REPORTS is the one its own decision is taken on, so the deny is
        // appended to the document the topic itself carries, as its owner would, and the repair
        // is the removal of that one statement.
        let repaired = reported_topic_policy(&router, case, region, &denied).await;
        let mut with_deny = repaired.clone();
        let Some(statements) = with_deny["Statement"].as_array_mut() else {
            panic!("case {case:?}: the reported Policy states no Statement array: {repaired}");
        };
        statements.push(deny_cloudwatch_publish(&denied));
        set_topic_policy(&router, case, region, &denied, &with_deny).await;

        let alarm = "connections-high";
        let instance = format!("{alarm}-no-such-instance");
        let request = merged(
            breaching_alarm(alarm, &instance, &[&allowed, &denied]),
            &patch,
        );
        let namespace = request["Namespace"].as_str().unwrap_or_default().to_string();
        let metric_name = request["MetricName"].as_str().unwrap_or_default().to_string();
        if namespace == "AWS/RDS" {
            assert_no_such_db_instance(&router, case, region, &instance).await;
        }
        cw(&router, case, region, "PutMetricAlarm", &request).await;

        // From here until the notification arrives, the only service called is SQS: what shows
        // up is the clock's own delivery and not some later read's.
        let arrived =
            delivered_without_calling_cloudwatch(&router, case, region, &url, 1).await;
        assert_eq!(
            arrived.len(),
            1,
            "case {case:?}: the clock's transition must deliver exactly one notification with no \
             further CloudWatch call, got {arrived:?}"
        );
        let envelope = &arrived[0];
        assert_eq!(
            envelope["TopicArn"].as_str(),
            Some(allowed.as_str()),
            "case {case:?}: only the topic that denies nothing may deliver, got {envelope}"
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
        assert_eq!(
            message["Trigger"]["Namespace"].as_str(),
            Some(namespace.as_str()),
            "case {case:?}: the Trigger must report the series the alarm evaluated, got {message}"
        );
        assert_eq!(
            message["Trigger"]["MetricName"].as_str(),
            Some(metric_name.as_str()),
            "case {case:?}: the Trigger must report the metric the alarm evaluated, got {message}"
        );

        // Both actions of that one transition are recorded, each decided on its own topic.
        let items = polled_action_items(&router, case, region, alarm, 2).await;
        assert_eq!(
            items.len(),
            2,
            "case {case:?}: both actions of the transition must be recorded, got {items:?}"
        );
        for (leg, topic, published) in [
            ("the topic whose policy says nothing about this publish", &allowed, true),
            ("the topic whose policy denies it", &denied, false),
        ] {
            let (_, item) = items
                .iter()
                .find(|(resource, _)| resource == topic)
                .unwrap_or_else(|| panic!("case {case:?}: no Action item for {topic}: {items:?}"));
            assert_action_item(&format!("{case} / {leg}"), item, topic, published);
        }

        // The state the evaluation reached is the state a client reads back, and the transition
        // itself is in the history beside the actions it fired.
        let state = reported_state(&router, case, region, alarm).await;
        assert_eq!(
            state, "ALARM",
            "case {case:?}: the alarm must report the state it moved to, got {state:?}"
        );
        let transitions = history_items(&router, case, region, alarm, "StateUpdate").await;
        assert!(
            transitions.iter().any(|item| item["HistoryData"]
                .as_str()
                .is_some_and(|data| data.contains("ALARM"))),
            "case {case:?}: the transition must be recorded as a StateUpdate item, got \
             {transitions:?}"
        );

        // The alarm goes on being evaluated, and its state does not change again — so nothing
        // is published again and nothing is recorded again, however many wake-ups pass.
        let late = arrivals_over(&router, case, region, &url, SETTLED_WATCH).await;
        assert!(
            late.is_empty(),
            "case {case:?}: a state that did not change again may deliver nothing again, and the \
             denied topic nothing at all, got {late:?}"
        );
        let settled = action_items(&router, case, region, alarm).await;
        assert_eq!(
            settled.len(),
            2,
            "case {case:?}: the settled alarm must still record exactly the two actions its one \
             transition fired, got {settled:?}"
        );

        // The deny was the ONLY thing suppressing that action: with the statement removed, a
        // fresh alarm's action to the same topic is executed and delivered into the same queue.
        set_topic_policy(&router, case, region, &denied, &repaired).await;
        let fresh = "connections-high-again";
        let fresh_instance = format!("{fresh}-no-such-instance");
        let fresh_request = merged(
            breaching_alarm(fresh, &fresh_instance, &[&denied]),
            &patch,
        );
        cw(&router, case, region, "PutMetricAlarm", &fresh_request).await;
        let arrived = delivered_without_calling_cloudwatch(&router, case, region, &url, 1).await;
        let sources: Vec<&str> = arrived
            .iter()
            .map(|envelope| envelope["TopicArn"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            sources,
            vec![denied.as_str()],
            "case {case:?}: the repaired topic must deliver the fresh alarm's notification, got \
             {arrived:?}"
        );
        let items = polled_action_items(&router, case, region, fresh, 1).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the fresh alarm's action must be recorded, got {items:?}"
        );
        assert_action_item(
            &format!("{case} / the deny removed"),
            &items[0].1,
            &denied,
            true,
        );
    }
}
