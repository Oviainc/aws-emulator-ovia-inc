// SPDX-License-Identifier: CC0-1.0

//! DELIVERY-STATUS LOGGING: what a topic carrying a `<Protocol>` feedback role writes about
//! each delivery it attempted, end to end over the wire through the real `MultiServiceRouter`,
//! with Amazon SQS, IAM and CloudWatch Logs mounted on the same engine as SNS — so the record
//! is read back where a client reads it, through `logs:DescribeLogStreams` and
//! `logs:GetLogEvents`.
//!
//! The RULE asserted here: "`<Protocol>`SuccessFeedbackRoleArn – Indicates successful message
//! delivery status for an Amazon SNS topic that is subscribed to an endpoint of that kind …
//! `<Protocol>`FailureFeedbackRoleArn – Indicates failed message delivery status …
//! `<Protocol>`SuccessFeedbackSampleRate – Indicates percentage of successful messages to
//! sample" (`SetTopicAttributesInput$AttributeName`) — a topic that names a feedback role for
//! the protocol a delivery went over has SNS ASSUME that role and write one record per
//! delivery into the CloudWatch Logs log group `sns/<region>/<account-id>/<topic-name>`,
//! naming the endpoint delivered to and the `SUCCESS`/`FAILURE` of the attempt; a success is
//! written at the sampled percentage and a failure always. A topic that names no role for the
//! delivery's protocol writes nothing, and neither does one whose role does not TRUST this
//! service, because there is then nothing to assume.
//!
//! The rows differ along what the rule generalises over: which of the feedback attributes the
//! topic carries, the sample rate (a full sample, none, and none named at all), the protocol
//! the attributes were set for as against the one the delivery went over, whether the delivery
//! succeeded or was withheld by the endpoint's own policy, and whether the named role's trust
//! policy admits this service.
//!
//! The deliveries here go over the `sqs` protocol, whose endpoint is a queue, because the
//! feedback configuration is per protocol and the queue's own `Policy` is the cleanest way to
//! make one delivery succeed and another fail; the `lambda` protocol's own deliveries are
//! `lambda_subscription_delivery.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sqs_subscription_delivery::element;
use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for, and so the Region every ARN here names.
const REGION: &str = "us-east-1";

/// The service principal a subscribed endpoint's own policy has to grant for a notification to
/// reach it, and the one a feedback role has to trust for SNS to assume it.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// How long a row waits for a record that is owed. A record is written as soon as the delivery
/// it reports has been attempted, so this is many writes' worth of slack rather than a span the
/// behaviour depends on.
const RECORD_BUDGET: Duration = Duration::from_secs(10);

/// How long a row waits before concluding that NOTHING was written.
const QUIET_BUDGET: Duration = Duration::from_millis(1_500);

/// How often a wait re-reads the log group it is waiting on.
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// SNS with the three services a logged delivery spans, on ONE engine: the queue the
/// notification is delivered to, the IAM store the feedback role is resolved in, and the
/// CloudWatch Logs store the record lands in.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    Emulator { router }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. Neither the key id nor the signature is a credential: this harness turns
/// request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded (awsQuery) call that must SUCCEED, as its body text — the shape both SNS
/// and IAM take, differing only in the signing name and the API version.
async fn query(
    emulator: &Emulator,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        // The form encoding is the suite-shared one, not a second copy.
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "{signing_name} {action}: expected HTTP 200, got {status} / {text}"
    );
    text
}

/// One SNS call that must succeed.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    query(emulator, "sns", "2010-03-31", action, params).await
}

/// One IAM call that must succeed.
async fn iam(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    query(emulator, "iam", "2010-05-08", action, params).await
}

/// One `awsJson1_0` call against SQS that must succeed.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "SQS {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One `awsJson1_1` call against CloudWatch Logs, with whatever status it answered: a row that
/// expects NO record must be able to see the group simply not being there.
async fn logs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header("authorization", authorization("logs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The trust policy a delivery-status feedback role carries: `sts:AssumeRole` for `trusted`,
/// which is what SNS has to be granted to assume the role and write the record.
fn trusts(trusted: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": trusted},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create the role `name` with a trust policy admitting `trusted`, and answer its ARN.
async fn given_role(emulator: &Emulator, name: &str, trusted: &str) -> String {
    let answered = iam(
        emulator,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trusts(trusted)),
        ],
    )
    .await;
    element(&answered, "Arn")
}

/// The queue policy AWS documents for an SQS queue subscribed to an SNS topic, which is what
/// makes a delivery to it SUCCEED; a queue carrying none has granted this service nothing, so
/// its delivery FAILS.
fn queue_grants_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": SNS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// A queue called `name`, granting SNS the send when `grants` asks for it: its url and the ARN
/// a subscription names it by.
async fn given_queue(emulator: &Emulator, name: &str, grants: bool) -> (String, String) {
    let created = sqs(emulator, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string();
    if grants {
        sqs(
            emulator,
            "SetQueueAttributes",
            &json!({
                "QueueUrl": url,
                "Attributes": {"Policy": queue_grants_sns(&arn)},
            }),
        )
        .await;
    }
    (url, arn)
}

/// The log group a topic's delivery-status records go into, read off the topic's own ARN:
/// `sns/<region>/<account-id>/<topic-name>`.
fn feedback_log_group(topic_arn: &str) -> String {
    let fields: Vec<&str> = topic_arn.split(':').collect();
    let [_, _, _, region, account, name] = fields[..] else {
        panic!("a topic ARN has six colon-separated fields, got {topic_arn}");
    };
    format!("sns/{region}/{account}/{name}")
}

/// Every delivery-status record `log_group` holds, parsed, across every stream in it — and an
/// empty list for a group that is not there at all.
async fn records_in(emulator: &Emulator, log_group: &str) -> Vec<Value> {
    let (status, streams) = logs(
        emulator,
        "DescribeLogStreams",
        &json!({"logGroupName": log_group}),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    let mut records: Vec<Value> = Vec::new();
    for stream in streams["logStreams"].as_array().cloned().unwrap_or_default() {
        let Some(name) = stream["logStreamName"].as_str() else {
            continue;
        };
        let (status, events) = logs(
            emulator,
            "GetLogEvents",
            &json!({
                "logGroupName": log_group,
                "logStreamName": name,
                "startFromHead": true,
            }),
        )
        .await;
        if status != 200 {
            continue;
        }
        records.extend(
            events["events"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|event| event["message"].as_str())
                .filter_map(|message| serde_json::from_str::<Value>(message).ok()),
        );
    }
    records
}

/// Wait until `log_group` holds a record, and answer the ones it held — or an empty list once
/// `budget` is spent.
async fn records_within(emulator: &Emulator, log_group: &str, budget: Duration) -> Vec<Value> {
    let deadline = Instant::now() + budget;
    loop {
        let records = records_in(emulator, log_group).await;
        if !records.is_empty() || Instant::now() >= deadline {
            return records;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Stands in for the feedback role's ARN in a row's attribute values, which is only known once
/// the role has been created.
const ROLE_PLACEHOLDER: &str = "<role>";

/// What one row configures, and what must come of it.
struct Row {
    case: &'static str,
    /// The feedback attributes the topic is given, as `(name, value)` pairs — the role ARN is
    /// substituted for [`ROLE_PLACEHOLDER`].
    attributes: &'static [(&'static str, &'static str)],
    /// The service principal the named role's trust policy admits.
    trusted: &'static str,
    /// Whether the subscribed queue grants this service the send, which is what decides whether
    /// the delivery succeeds.
    queue_grants: bool,
    /// The `status` the record must report, and `None` for a configuration that must write no
    /// record at all.
    status: Option<&'static str>,
}

/// The attribute names are this service's own closed set for the delivery-status feature
/// (`SetTopicAttributesInput$AttributeName`), whose per-protocol prefixes are `Application`,
/// `Firehose`, `HTTP`, `Lambda` and `SQS`; the rows below exercise the `SQS` pair against an
/// `sqs` delivery and the `Lambda` pair against it, which must log nothing.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a full sample of successes over the delivery's own protocol",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            trusted: SNS_PRINCIPAL,
            queue_grants: true,
            status: Some("SUCCESS"),
        },
        Row {
            case: "a failure role, and a delivery the endpoint's own policy withheld",
            attributes: &[("SQSFailureFeedbackRoleArn", ROLE_PLACEHOLDER)],
            trusted: SNS_PRINCIPAL,
            queue_grants: false,
            status: Some("FAILURE"),
        },
        Row {
            case: "a success role but no sample of successes",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "0"),
            ],
            trusted: SNS_PRINCIPAL,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a success role with no sample rate named at all",
            attributes: &[("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER)],
            trusted: SNS_PRINCIPAL,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a full sample, but set for ANOTHER protocol",
            attributes: &[
                ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "100"),
            ],
            trusted: SNS_PRINCIPAL,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a role whose trust policy does not admit this service",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            trusted: "events.amazonaws.com",
            queue_grants: true,
            status: None,
        },
        Row {
            case: "no feedback attribute at all",
            attributes: &[],
            trusted: SNS_PRINCIPAL,
            queue_grants: true,
            status: None,
        },
    ]
}

#[tokio::test]
async fn a_topic_with_a_feedback_role_logs_one_record_per_delivery() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "feedback")]).await,
            "TopicArn",
        );
        let role = given_role(&emulator, &format!("sns-feedback-{index}"), row.trusted).await;
        let (_, endpoint) =
            given_queue(&emulator, &format!("feedback-{index}"), row.queue_grants).await;
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &endpoint),
            ],
        )
        .await;
        for (name, value) in row.attributes {
            let value = value.replace(ROLE_PLACEHOLDER, &role);
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", name),
                    ("AttributeValue", &value),
                ],
            )
            .await;
        }

        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", "a notification")],
        )
        .await;

        let log_group = feedback_log_group(&topic);
        let budget = if row.status.is_some() {
            RECORD_BUDGET
        } else {
            QUIET_BUDGET
        };
        let records = records_within(&emulator, &log_group, budget).await;
        let Some(expected) = row.status else {
            assert!(
                records.is_empty(),
                "case {case:?}: this topic asks for no delivery-status record, so {log_group} \
                 must stay empty, got {records:?}"
            );
            continue;
        };
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected one delivery-status record in {log_group}, got {records:?}"
        );
        let record = &records[0];
        assert_eq!(
            record["status"].as_str(),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {record}"
        );
        assert_eq!(
            record["delivery"]["destination"].as_str(),
            Some(endpoint.as_str()),
            "case {case:?}: the record must name the endpoint delivered to, got {record}"
        );
        assert_eq!(
            record["notification"]["topicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the record must name the topic published to, got {record}"
        );
        assert!(
            record["notification"]["messageId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the record must name the published message, got {record}"
        );
        assert!(
            record["delivery"]["deliveryId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the record must carry a delivery id, got {record}"
        );
    }
}

/// RULE: one record per DELIVERY — so a topic with two subscribed endpoints logs one record
/// naming each of them, and a second publish logs one more per endpoint.
#[tokio::test]
async fn one_record_is_written_per_delivery_of_the_topic() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "fan-out")]).await,
        "TopicArn",
    );
    let role = given_role(&emulator, "sns-fan-out-feedback", SNS_PRINCIPAL).await;
    let (_, first) = given_queue(&emulator, "fan-out-one", true).await;
    let (_, second) = given_queue(&emulator, "fan-out-two", true).await;
    for endpoint in [&first, &second] {
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }
    for (name, value) in [
        ("SQSSuccessFeedbackRoleArn", role.as_str()),
        ("SQSSuccessFeedbackSampleRate", "100"),
    ] {
        sns(
            &emulator,
            "SetTopicAttributes",
            &[
                ("TopicArn", &topic),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    }

    let log_group = feedback_log_group(&topic);
    // The rows differ in how many publishes precede the read: each one is delivered to both
    // endpoints, so the group holds two records per publish.
    for (case, publishes, expected) in [("one publish", 1, 2), ("a second publish", 2, 4)] {
        if case == "a second publish" {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", "again")],
            )
            .await;
        } else {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", "first")],
            )
            .await;
        }
        let deadline = Instant::now() + RECORD_BUDGET;
        let mut records = records_in(&emulator, &log_group).await;
        while records.len() < expected && Instant::now() < deadline {
            tokio::time::sleep(LOOK_INTERVAL).await;
            records = records_in(&emulator, &log_group).await;
        }
        assert_eq!(
            records.len(),
            expected,
            "case {case:?}: expected {expected} record(s) after {publishes} publish(es), got \
             {records:?}"
        );
        let destinations: Vec<&str> = records
            .iter()
            .filter_map(|record| record["delivery"]["destination"].as_str())
            .collect();
        for endpoint in [&first, &second] {
            assert!(
                destinations.contains(&endpoint.as_str()),
                "case {case:?}: expected a record naming {endpoint}, got {destinations:?}"
            );
        }
    }
}
