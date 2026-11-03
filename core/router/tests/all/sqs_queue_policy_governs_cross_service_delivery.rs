// SPDX-License-Identifier: CC0-1.0

//! An Amazon SQS queue takes a delivery made by ANOTHER SERVICE only where the queue's own
//! `Policy` document grants that service's principal `sqs:SendMessage` on it — asserted over
//! the production service set (`aws_emulator_router::build_router` on one engine, exactly as
//! the binary assembles it), where a client observes it: on the queue.
//!
//! The RULE pinned here: a cross-service delivery into a queue is performed as the
//! DELIVERING service's own service principal, which holds no identity-based policies, so
//! the queue's resource policy is the only document in force over it — nothing is enqueued
//! unless a statement of that document allows that principal this action on this queue ARN,
//! an explicit `Deny` about it refuses the delivery whatever else is granted, and a grant
//! scoped by `aws:SourceArn`/`aws:SourceAccount` reaches only the deliveries whose source
//! those keys name. EventBridge states it for a queue target ("for Lambda functions, Amazon
//! SNS topics and Amazon SQS queues, EventBridge relies on resource-based policies",
//! `PutTargets`), and Amazon SNS for a subscribed queue ("you must ensure that the Amazon
//! SQS queue policy allows the Amazon SNS topic to send messages to the queue").
//!
//! What this suite adds to each service's own cover is the CROSSING: one queue, wired to
//! both a rule and a topic at once, so each row shows which of the two deliveries a document
//! admits. A grant to one service principal is not a grant to the other, and a grant scoped
//! to one delivery's source admits that delivery alone — neither of which a single service's
//! suite can observe.
//!
//! The rows differ along what the rule generalises over: whether the queue carries a
//! document at all, which principal it names (one service, both, every principal), which
//! action, which resource, which `Effect` and in which order, and which condition key and
//! value the grant is scoped by. Each row runs on a FRESH emulator, so one row's document
//! never decides another row's delivery.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, SERVICE_PRINCIPAL as EVENTS_PRINCIPAL,
};

use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The service principal Amazon SNS delivers to a subscribed queue as, which is the
/// identity a queue policy has to grant for a notification to reach it. The EventBridge
/// one is the delivering crate's own published constant ([`EVENTS_PRINCIPAL`]).
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The message every notification below carries, which is what an SNS envelope on the queue
/// is recognised by.
const NOTIFICATION_MESSAGE: &str = "{\"probe\":\"notification\"}";

/// The IAM action a delivery into a queue performs on it, whichever service makes it.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The `source` every event this suite publishes carries, and what its rule matches on.
const SOURCE: &str = "queue.policy.probe";

/// The `detail-type` of that event, which is what tells an EventBridge delivery apart from
/// an SNS notification on the queue.
const DETAIL_TYPE: &str = "QueuePolicyProbe";

/// The one rule each row publishes through, named so a row's condition can scope a grant to
/// it.
const RULE: &str = "queue-policy-rule";

/// The whole emulator over one engine, exactly as the binary assembles it — so which
/// service claims each call, EventBridge's and SNS's own in-process calls into Amazon SQS
/// included, is decided by the production mount rather than by a hand-picked set.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Amazon SNS awsQuery call (API version 2010-03-31) that must answer 200, as its XML
/// body.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SNS {action}, got {status} with {text}"
    );
    text
}

/// A queue called `name` carrying NO policy of its own: its url and the ARN it reports.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let reported = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = reported["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {reported}"))
        .to_string();
    (url, arn)
}

/// A topic called `name` with the queue `queue_arn` subscribed to it: its ARN.
async fn given_topic_subscribed(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queue_arn: &str,
) -> String {
    let created = sns(router, case, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = element(&created, "TopicArn");
    sns(
        router,
        case,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    topic_arn
}

/// A rule on the default bus matching this suite's event, delivering to `queue_arn`.
async fn given_rule(router: &MultiServiceRouter, case: &str, queue_arn: &str) {
    events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventPattern": json!({
                "source": [SOURCE],
                "detail-type": [DETAIL_TYPE],
            })
            .to_string(),
        }),
    )
    .await;
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "Targets": [{ "Id": "queue", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: the queue target must be accepted, got {targeted}"
    );
}

/// Write `policy` as the `Policy` attribute of the queue at `url`. An empty value is how
/// Amazon SQS clears an attribute, which is what withdrawing a queue policy writes.
async fn set_policy(router: &MultiServiceRouter, case: &str, url: &str, policy: &str) {
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
}

/// Publish the one event this suite's rule matches.
async fn publish_event(router: &MultiServiceRouter, case: &str) {
    let published = events(
        router,
        case,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "probe": case }).to_string(),
            }]
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: the event must be accepted, got {published}"
    );
}

/// Publish one notification on `topic_arn`.
async fn publish_notification(router: &MultiServiceRouter, case: &str, topic_arn: &str) {
    sns(
        router,
        case,
        "Publish",
        &[("TopicArn", topic_arn), ("Message", NOTIFICATION_MESSAGE)],
    )
    .await;
}

/// Every message body waiting on the queue at `url`, DELETED as it is read: each stage below
/// counts what one publication left on the queue, so nothing a previous stage received may
/// come back into view for the next one.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut bodies = Vec::with_capacity(messages.len());
    for message in &messages {
        let handle = message["ReceiptHandle"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ReceiptHandle, got {message}"));
        sqs(
            router,
            case,
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
        )
        .await;
        bodies.push(message["Body"].as_str().unwrap_or_default().to_string());
    }
    bodies
}

/// Everything a row's document can name: the queue it is installed on, and the source of
/// each of the two deliveries it decides.
struct Wiring {
    queue_arn: String,
    topic_arn: String,
    rule_arn: String,
    account: String,
}

/// The account an ARN this emulator minted belongs to.
fn account_of(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("no account field in {arn}"))
        .to_string()
}

/// A one-or-many statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A statement about `action` on `resource` for `principal`, scoped by `condition` when the
/// row states one.
fn statement(
    effect: &str,
    principal: Value,
    action: Value,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// The `Principal` element naming both delivering services.
fn both_services() -> Value {
    json!({ "Service": [EVENTS_PRINCIPAL, SNS_PRINCIPAL] })
}

/// An `Allow` of [`SEND_MESSAGE`] to both delivering services on the row's own queue,
/// scoped by `condition` when one is given.
fn grant_to_both(wiring: &Wiring, condition: Option<Value>) -> String {
    document(json!([statement(
        "Allow",
        both_services(),
        json!(SEND_MESSAGE),
        &wiring.queue_arn,
        condition,
    )]))
}

/// One row of the table: the document the queue carries, and how many messages each of the
/// two deliveries is to leave on it.
struct Row {
    case: &'static str,
    /// `None` is a queue carrying no `Policy` attribute at all.
    policy: Option<fn(&Wiring) -> String>,
    from_events: usize,
    from_sns: usize,
}

/// The queue's own document decides each cross-service delivery, per delivering principal.
#[tokio::test]
async fn a_queue_takes_a_cross_service_delivery_only_where_its_policy_grants_that_service() {
    let rows: &[Row] = &[
        Row {
            case: "no policy at all, which grants nobody",
            policy: None,
            from_events: 0,
            from_sns: 0,
        },
        Row {
            case: "a grant to the EventBridge principal alone",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!({ "Service": EVENTS_PRINCIPAL }),
                    json!(SEND_MESSAGE),
                    &wiring.queue_arn,
                    None,
                )]))
            }),
            from_events: 1,
            from_sns: 0,
        },
        Row {
            case: "a grant to the Amazon SNS principal alone",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!({ "Service": SNS_PRINCIPAL }),
                    json!(SEND_MESSAGE),
                    &wiring.queue_arn,
                    None,
                )]))
            }),
            from_events: 0,
            from_sns: 1,
        },
        Row {
            case: "one grant naming both service principals",
            policy: Some(|wiring| grant_to_both(wiring, None)),
            from_events: 1,
            from_sns: 1,
        },
        Row {
            case: "a grant to every principal",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!("*"),
                    json!(SEND_MESSAGE),
                    &wiring.queue_arn,
                    None,
                )]))
            }),
            from_events: 1,
            from_sns: 1,
        },
        Row {
            case: "a grant of every action of the service to both",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    both_services(),
                    json!("sqs:*"),
                    &wiring.queue_arn,
                    None,
                )]))
            }),
            from_events: 1,
            from_sns: 1,
        },
        Row {
            case: "a grant to both of another action of the same service",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    both_services(),
                    json!("sqs:ReceiveMessage"),
                    &wiring.queue_arn,
                    None,
                )]))
            }),
            from_events: 0,
            from_sns: 0,
        },
        Row {
            case: "a grant to both whose Resource names another queue",
            policy: Some(|wiring| {
                let other = format!("{}-elsewhere", wiring.queue_arn);
                document(json!([statement(
                    "Allow",
                    both_services(),
                    json!(SEND_MESSAGE),
                    &other,
                    None,
                )]))
            }),
            from_events: 0,
            from_sns: 0,
        },
        Row {
            case: "a grant to both with an explicit deny of the EventBridge principal \
                   written after it",
            policy: Some(|wiring| {
                document(json!([
                    statement(
                        "Allow",
                        both_services(),
                        json!(SEND_MESSAGE),
                        &wiring.queue_arn,
                        None,
                    ),
                    statement(
                        "Deny",
                        json!({ "Service": EVENTS_PRINCIPAL }),
                        json!(SEND_MESSAGE),
                        &wiring.queue_arn,
                        None,
                    ),
                ]))
            }),
            from_events: 0,
            from_sns: 1,
        },
        Row {
            case: "a grant to both with an explicit deny of the Amazon SNS principal \
                   written before it",
            policy: Some(|wiring| {
                document(json!([
                    statement(
                        "Deny",
                        json!({ "Service": SNS_PRINCIPAL }),
                        json!(SEND_MESSAGE),
                        &wiring.queue_arn,
                        None,
                    ),
                    statement(
                        "Allow",
                        both_services(),
                        json!(SEND_MESSAGE),
                        &wiring.queue_arn,
                        None,
                    ),
                ]))
            }),
            from_events: 1,
            from_sns: 0,
        },
        Row {
            case: "a grant to both scoped to this account by aws:SourceAccount",
            policy: Some(|wiring| {
                grant_to_both(
                    wiring,
                    Some(json!({ "StringEquals": { "aws:SourceAccount": wiring.account } })),
                )
            }),
            from_events: 1,
            from_sns: 1,
        },
        Row {
            case: "a grant to both scoped to another account by aws:SourceAccount",
            policy: Some(|wiring| {
                grant_to_both(
                    wiring,
                    Some(json!({ "StringEquals": { "aws:SourceAccount": "210987654321" } })),
                )
            }),
            from_events: 0,
            from_sns: 0,
        },
        Row {
            case: "a grant to both scoped by aws:SourceArn to the publishing rule",
            policy: Some(|wiring| {
                grant_to_both(
                    wiring,
                    Some(json!({ "ArnEquals": { "aws:SourceArn": wiring.rule_arn } })),
                )
            }),
            from_events: 1,
            from_sns: 0,
        },
        Row {
            case: "a grant to both scoped by aws:SourceArn to the publishing topic",
            policy: Some(|wiring| {
                grant_to_both(
                    wiring,
                    Some(json!({ "ArnEquals": { "aws:SourceArn": wiring.topic_arn } })),
                )
            }),
            from_events: 0,
            from_sns: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router();
        let (url, queue_arn) = given_queue(&router, case, &format!("policy-case-{index}")).await;
        let topic_arn = given_topic_subscribed(
            &router,
            case,
            &format!("policy-topic-{index}"),
            &queue_arn,
        )
        .await;
        given_rule(&router, case, &queue_arn).await;
        let wiring = Wiring {
            account: account_of(&queue_arn),
            rule_arn: rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE),
            queue_arn,
            topic_arn,
        };
        if let Some(policy) = row.policy {
            set_policy(&router, case, &url, &policy(&wiring)).await;
        }

        publish_event(&router, case).await;
        let from_events = drain(&router, case, &url).await;
        assert_eq!(
            from_events.len(),
            row.from_events,
            "case {case:?}: expected {} message(s) from the rule's delivery, got \
             {from_events:?}",
            row.from_events
        );
        for body in &from_events {
            let envelope: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            assert_eq!(
                envelope["detail-type"].as_str(),
                Some(DETAIL_TYPE),
                "case {case:?}: what arrived must be the routed event, got {body:?}"
            );
        }

        publish_notification(&router, case, &wiring.topic_arn).await;
        let from_sns = drain(&router, case, &url).await;
        assert_eq!(
            from_sns.len(),
            row.from_sns,
            "case {case:?}: expected {} message(s) from the subscription's delivery, got \
             {from_sns:?}",
            row.from_sns
        );
        for body in &from_sns {
            let envelope: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            assert_eq!(
                envelope["Message"].as_str(),
                Some(NOTIFICATION_MESSAGE),
                "case {case:?}: what arrived must be the published notification, got {body:?}"
            );
        }
    }
}

/// Each delivery is decided against the document the queue carries AT THAT INSTANT, and the
/// same document governs both delivering services: with none, neither reaches the queue; a
/// grant written to both lets the next delivery of each through; clearing it stops the one
/// after that.
#[tokio::test]
async fn a_queue_policy_decides_every_cross_service_delivery_as_it_stands_then() {
    let case = "a grant to both services written and then cleared";
    let router = router();
    let (url, queue_arn) = given_queue(&router, case, "late-grant").await;
    let topic_arn = given_topic_subscribed(&router, case, "late-grant-topic", &queue_arn).await;
    given_rule(&router, case, &queue_arn).await;
    let wiring = Wiring {
        account: account_of(&queue_arn),
        rule_arn: rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE),
        queue_arn,
        topic_arn,
    };

    for (stage, policy, expected) in [
        ("before any policy is written", None, 0usize),
        ("once the grant is written", Some(grant_to_both(&wiring, None)), 1),
        ("once the grant is cleared", Some(String::new()), 0),
    ] {
        if let Some(policy) = policy {
            set_policy(&router, case, &url, &policy).await;
        }
        publish_event(&router, case).await;
        let from_events = drain(&router, case, &url).await;
        assert_eq!(
            from_events.len(),
            expected,
            "case {case:?} {stage}: expected {expected} message(s) from the rule's delivery, \
             got {from_events:?}"
        );
        publish_notification(&router, case, &wiring.topic_arn).await;
        let from_sns = drain(&router, case, &url).await;
        assert_eq!(
            from_sns.len(),
            expected,
            "case {case:?} {stage}: expected {expected} message(s) from the subscription's \
             delivery, got {from_sns:?}"
        );
    }
}
