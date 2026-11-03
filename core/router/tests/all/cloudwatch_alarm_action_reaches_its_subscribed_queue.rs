// SPDX-License-Identifier: CC0-1.0

//! An alarm's SNS action reaches the queue subscribed to its topic — through the WHOLE
//! mounted service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a metric alarm whose `AlarmActions` names an SNS topic publishes
//! the alarm notification to that topic on every transition INTO the ALARM state, so a queue
//! subscribed to the topic (and granting the SNS service principal the send) receives one
//! message per transition whose body carries the alarm's name and `NewStateValue`; while the
//! alarm's `ActionsEnabled` is false the transition still happens and the publish is
//! withheld, and `EnableAlarmActions` restores it.**
//!
//! Three services have to agree for that to hold — CloudWatch publishes, SNS fans out, SQS
//! holds what arrived — and the publish CloudWatch makes is an in-process call addressed the
//! way a client's own `sns`-scoped call is addressed, so WHICH mounted model claims it is
//! only decidable with every service mounted. A crate-local suite mounts the three it needs
//! and can therefore never see a fourth service claiming that call, nor the key resolving to
//! none: `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/tests/all/`
//! pins the dispatch, its `ActionsEnabled` gate and the policy decisions over tables of
//! alarms, Regions and grants; what only this suite can reach is the same path over the
//! production service set.
//!
//! The rows differ along what the rule generalises over: how the subscribed queue's policy
//! scopes the grant (unscoped, or conditioned on the publishing topic) and the Region the
//! whole wiring lives in.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// CloudWatch's `X-Amz-Target` prefix and its signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The service principal SNS performs its delivery to a subscribed queue as, which is what
/// that queue's own policy has to allow.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// How many times a leg polls the queue before calling the notification undelivered, and the
/// long poll each of those calls makes. Driven off what has ARRIVED rather than off a fixed
/// sleep, so a slow machine delays a leg instead of breaking it.
const POLLS: usize = 10;
const WAIT_TIME_SECONDS: u32 = 1;

/// The whole emulator over one engine, exactly as the binary assembles it.
pub(crate) fn router() -> MultiServiceRouter {
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
pub(crate) async fn cw(
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
async fn sqs(router: &MultiServiceRouter, case: &str, region: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, region, "sqs", "AmazonSQS", operation, body).await
}

/// One SNS call over the `awsQuery` form wire that service serves, which must answer 200.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{region}.amazonaws.com"))
        .header("authorization", authorization("sns", region))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {text}"
    );
    text
}

/// How the subscribed queue's policy grants SNS the send: the documented grant ("you must
/// ensure that the Amazon SQS queue policy allows the Amazon SNS topic to send messages to
/// the queue"), and whether it is scoped by condition to the publishing topic.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Grant {
    Unscoped,
    ScopedToTopic,
}

impl Grant {
    fn policy(self, queue_arn: &str, topic_arn: &str) -> String {
        let mut statement = json!({
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": SNS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        });
        if matches!(self, Self::ScopedToTopic) {
            statement["Condition"] = json!({"ArnEquals": {"aws:SourceArn": topic_arn}});
        }
        json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
    }
}

/// A topic in `region` with a queue of this emulator subscribed to it, the queue granting SNS
/// the send the way `grant` writes it. Answers the topic's ARN and the queue's url.
pub(crate) async fn given_topic_with_subscriber(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    grant: Grant,
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
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    let topic = element(
        &sns(router, case, region, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    sqs(
        router,
        case,
        region,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": grant.policy(&queue_arn, &topic)},
        }),
    )
    .await;
    sns(
        router,
        case,
        region,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;
    (topic, url)
}

/// Take every notification waiting on the queue at `url`, as the alarm documents SNS
/// enveloped, deleting what it hands over so a later leg reads only what arrived since.
pub(crate) async fn drain(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        region,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "WaitTimeSeconds": WAIT_TIME_SECONDS,
        }),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut notifications = Vec::with_capacity(messages.len());
    for message in &messages {
        let body = message["Body"].as_str().unwrap_or_default();
        let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("case {case:?}: the body is not the SNS envelope ({error}): {body}")
        });
        let inner = envelope["Message"].as_str().unwrap_or_default();
        notifications.push(serde_json::from_str(inner).unwrap_or_else(|error| {
            panic!("case {case:?}: the message is not an alarm notification ({error}): {inner}")
        }));
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                case,
                region,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
    notifications
}

/// Drain until `wanted` notifications have arrived or the polls run out.
pub(crate) async fn drain_until(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let mut arrived: Vec<Value> = Vec::new();
    for _ in 0..POLLS {
        if arrived.len() >= wanted {
            break;
        }
        arrived.extend(drain(router, case, region, url).await);
    }
    arrived
}

/// Force one alarm into `state`, which is what invokes the actions configured for it.
async fn force(router: &MultiServiceRouter, case: &str, region: &str, alarm: &str, state: &str) {
    cw(
        router,
        case,
        region,
        "SetAlarmState",
        &json!({
            "AlarmName": alarm,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// Turn the alarm's actions on or off, and answer what `DescribeAlarms` then reports.
async fn switch_actions(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    enabled: bool,
) -> Option<bool> {
    let operation = if enabled {
        "EnableAlarmActions"
    } else {
        "DisableAlarmActions"
    };
    cw(router, case, region, operation, &json!({"AlarmNames": [alarm]})).await;
    let described = cw(
        router,
        case,
        region,
        "DescribeAlarms",
        &json!({"AlarmNames": [alarm]}),
    )
    .await;
    described["MetricAlarms"][0]["ActionsEnabled"].as_bool()
}

#[tokio::test]
async fn an_alarms_sns_action_reaches_the_queue_subscribed_to_its_topic() {
    // (case, the Region the wiring lives in, how the queue grants SNS the send)
    let rows: &[(&str, &str, Grant)] = &[
        ("the grant scoped to the publishing topic", "us-west-2", Grant::ScopedToTopic),
        ("an unscoped grant in another Region", "eu-west-1", Grant::Unscoped),
    ];

    for (case, region, grant) in rows {
        let router = router();
        let (topic, url) =
            given_topic_with_subscriber(&router, case, region, "alarm-actions", *grant).await;

        let alarm = "errors-high";
        cw(
            &router,
            case,
            region,
            "PutMetricAlarm",
            &json!({
                "AlarmName": alarm,
                "Namespace": "Custom/RouterWiring",
                "MetricName": "Errors",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "missing",
                "AlarmActions": [topic.clone()],
            }),
        )
        .await;

        // The positive control: the actions are enabled by default, so the transition into
        // ALARM publishes to the topic and the subscribed queue receives it.
        force(&router, case, region, alarm, "ALARM").await;
        let delivered = drain_until(&router, case, region, &url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the alarm's action must reach the subscribed queue once, got \
             {delivered:?}"
        );
        assert_eq!(
            delivered[0]["AlarmName"].as_str(),
            Some(alarm),
            "case {case:?}: the notification must name the alarm, got {}",
            delivered[0]
        );
        assert_eq!(
            delivered[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the notification must carry the state moved to, got {}",
            delivered[0]
        );

        // With the actions off the state still moves and nothing is delivered. The force into
        // OK before each ALARM force is what makes that force a state CHANGE whatever the
        // alarm's own evaluation left behind; it fires nothing itself, this alarm carrying no
        // `OKActions`.
        let reported = switch_actions(&router, case, region, alarm, false).await;
        assert_eq!(
            reported,
            Some(false),
            "case {case:?}: DisableAlarmActions must turn the switch off, got {reported:?}"
        );
        force(&router, case, region, alarm, "OK").await;
        force(&router, case, region, alarm, "ALARM").await;
        let described = cw(
            &router,
            case,
            region,
            "DescribeAlarms",
            &json!({"AlarmNames": [alarm]}),
        )
        .await;
        assert_eq!(
            described["MetricAlarms"][0]["StateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the suppressed transition must still move the state, got {described}"
        );
        let withheld = drain(&router, case, region, &url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: nothing may be delivered while the actions are off, got {withheld:?}"
        );

        // And back on: the next transition into ALARM is delivered, and the one withheld while
        // the actions were off is not delivered late.
        let reported = switch_actions(&router, case, region, alarm, true).await;
        assert_eq!(
            reported,
            Some(true),
            "case {case:?}: EnableAlarmActions must turn the switch back on, got {reported:?}"
        );
        force(&router, case, region, alarm, "OK").await;
        force(&router, case, region, alarm, "ALARM").await;
        let restored = drain_until(&router, case, region, &url, 1).await;
        assert_eq!(
            restored.len(),
            1,
            "case {case:?}: the transition made with the actions back on must be delivered, and \
             only that one, got {restored:?}"
        );
        assert_eq!(
            restored[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the restored notification must carry the state moved to, got {}",
            restored[0]
        );
    }
}
