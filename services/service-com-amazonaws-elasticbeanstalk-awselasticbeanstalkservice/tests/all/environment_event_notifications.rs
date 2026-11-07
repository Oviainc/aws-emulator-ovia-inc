// SPDX-License-Identifier: CC0-1.0

//! An environment's events are DELIVERED, not only logged: every event this
//! service records about an environment whose configuration carries the
//! `aws:elasticbeanstalk:sns:topics` settings is published to that environment's
//! Amazon SNS topic, so a client subscribed to the topic sees the environment's
//! lifecycle without reading the event log.
//!
//! The rule under test: an environment configured with Elastic Beanstalk's
//! notification settings has every event the service records about it published
//! to its notification topic — the topic the configuration names, or the topic
//! Elastic Beanstalk creates and subscribes the configured endpoint to — and an
//! environment configured with none has nothing published for it.
//!
//! The table below spans what the rule generalises over: HOW the topic is
//! configured (an ARN the caller created it under, an endpoint alone, an endpoint
//! with the name to create the topic under, and no notification settings at all).
//! Both kinds of event are asserted for each configured row: the one the
//! lifecycle CALL records (`createEnvironment is starting.`) and the one the
//! CLOCK makes due later (the launch completion), because the two are recorded
//! through different paths.
//!
//! Every suite here mounts SNS and SQS beside Elastic Beanstalk on ONE engine and
//! reads the notification off a queue subscribed to the topic, so the delivery
//! asserted is the one a client receives — the publish goes through Amazon SNS's
//! own `Publish` and its own delivery to the subscribed queue.

use serde_json::{json, Value};

use crate::harness::{Answer, Emulator, PAST_DUE, SOLUTION_STACK};
use crate::lifecycle::{form_encoded, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every environment below is created under.
const APPLICATION: &str = "notified-app";

/// The environment every row creates.
const ENVIRONMENT: &str = "notified-env";

/// The queue the notification is read from, and the topic a row that names one
/// creates.
const QUEUE: &str = "eb-environment-events";

/// The API version SNS's awsQuery surface takes (`version: 2010-03-31`).
const SNS_API_VERSION: &str = "2010-03-31";

/// How many times a suite re-reads the queue while waiting for a delivery.
/// Delivery is asynchronous — the call that recorded the event publishes once it
/// has answered — so a test waits for the notification instead of assuming the
/// call that recorded the event also delivered it.
const DELIVERY_READS: usize = 40;

/// One awsQuery call against the SNS service mounted beside Elastic Beanstalk.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> Answer {
    let mut form = vec![
        format!("Action={action}"),
        format!("Version={SNS_API_VERSION}"),
    ];
    for (name, value) in params {
        form.push(format!("{}={}", form_encoded(name), form_encoded(value)));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{TEST_REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20261021/{TEST_REGION}/sns/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(form.join("&")))
        .expect("a well-formed request");
    let answer = emulator.dispatch(request).await;
    assert_eq!(
        answer.status, 200,
        "{action}: expected 200 from SNS, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// One awsJson call against the SQS service mounted beside Elastic Beanstalk.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{TEST_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20261021/{TEST_REGION}/sqs/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let answer = emulator.dispatch(request).await;
    assert_eq!(
        answer.status, 200,
        "{operation}: expected 200 from SQS, got {} and body {}",
        answer.status, answer.body
    );
    serde_json::from_str(&answer.body).unwrap_or(Value::Null)
}

/// The queue every row reads its notifications from: its URL and its ARN, with
/// the policy an SNS subscription needs to deliver into it already set.
async fn given_queue(emulator: &Emulator) -> (String, String) {
    let created = sqs(emulator, "CreateQueue", &json!({"QueueName": QUEUE})).await;
    let url = created["QueueUrl"]
        .as_str()
        .expect("CreateQueue answers a QueueUrl")
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .expect("GetQueueAttributes answers the QueueArn")
        .to_string();
    // The queue policy AWS documents for a queue subscribed to a topic: the
    // service principal `sns.amazonaws.com`, allowed `sqs:SendMessage` on it.
    sqs(
        emulator,
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
                    "Resource": arn,
                }],
            }).to_string()},
        }),
    )
    .await;
    (url, arn)
}

/// The messages the queue holds now, as the SNS notification envelopes they
/// carry.
async fn delivered(emulator: &Emulator, queue_url: &str) -> Vec<Value> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message.get("Body").and_then(Value::as_str).unwrap_or_default();
            serde_json::from_str(body)
                .unwrap_or_else(|_| panic!("the delivered body is an SNS envelope, got {body}"))
        })
        .collect()
}

/// Read the queue until it has delivered a notification whose `Message` carries
/// every one of `wanted`, driving this service's clock with a describe between
/// reads, and answer every envelope that was delivered.
///
/// The describe is what materialises a change the clock has made due — the launch
/// completion — exactly as a client polling for `Ready` does.
async fn await_delivery(emulator: &Emulator, queue_url: &str, wanted: &[&str]) -> Vec<Value> {
    let mut envelopes: Vec<Value> = Vec::new();
    for _ in 0..DELIVERY_READS {
        envelopes.extend(delivered(emulator, queue_url).await);
        if wanted
            .iter()
            .all(|want| envelopes.iter().any(|envelope| carries(envelope, want)))
        {
            break;
        }
        // The launch completion is due one status-transition span after the
        // create, and it is a describe that materialises it.
        emulator
            .call("DescribeEnvironments", &[("ApplicationName", APPLICATION)])
            .await;
        ::tokio::time::sleep(::std::time::Duration::from_millis(50)).await;
    }
    envelopes
}

/// Whether one delivered envelope's `Message` carries `text`.
fn carries(envelope: &Value, text: &str) -> bool {
    envelope
        .get("Message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.contains(text))
}

/// Seed the application every environment is created under.
async fn given_application(emulator: &Emulator) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "seeding {APPLICATION}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
}

/// Create the environment with `settings` as its configuration options.
async fn create_environment(
    emulator: &Emulator,
    case: &str,
    settings: &[(&str, &str)],
) -> Answer {
    let mut params = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", ENVIRONMENT),
        ("SolutionStackName", SOLUTION_STACK.as_str()),
    ];
    // The member indexes are built here so a row can name as many options as it
    // needs without writing the wire names out itself.
    let members: Vec<(String, String)> = settings
        .iter()
        .enumerate()
        .flat_map(|(index, (option, value))| {
            let at = index + 1;
            [
                (
                    format!("OptionSettings.member.{at}.Namespace"),
                    eb::state::NAMESPACE_SNS_TOPICS.to_string(),
                ),
                (
                    format!("OptionSettings.member.{at}.OptionName"),
                    (*option).to_string(),
                ),
                (
                    format!("OptionSettings.member.{at}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect();
    for (name, value) in &members {
        params.push((name.as_str(), value.as_str()));
    }
    let launched = emulator.call("CreateEnvironment", &params).await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    launched
}

/// The `Notification Topic ARN` the environment's configuration reports now.
async fn reported_topic_arn(emulator: &Emulator, case: &str) -> String {
    let described = emulator
        .call(
            "DescribeConfigurationSettings",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", ENVIRONMENT),
            ],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeConfigurationSettings, got {} and body {}",
        described.status, described.body
    );
    described
        .elements_at("ConfigurationSettings/member/OptionSettings/member")
        .iter()
        .find(|setting| {
            setting.value("Namespace") == Some(eb::state::NAMESPACE_SNS_TOPICS)
                && setting.value("OptionName") == Some(eb::state::OPTION_NOTIFICATION_TOPIC_ARN)
        })
        .and_then(|setting| setting.value("Value").map(str::to_string))
        .unwrap_or_default()
}

/// One way of configuring an environment's notifications.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// Whether the test creates the topic itself and names it by ARN.
    topic_named_by_arn: bool,
    /// Whether the configuration names the queue as the endpoint to subscribe.
    endpoint_configured: bool,
    /// The name the configuration asks the created topic to carry, if any.
    topic_name: Option<&'static str>,
    /// Whether the environment's events must reach the queue.
    delivered: bool,
}

/// One row per way the notifications are configured.
const ROWS: &[Row] = &[
    Row {
        case: "the configuration names the topic by ARN",
        topic_named_by_arn: true,
        endpoint_configured: false,
        topic_name: None,
        delivered: true,
    },
    Row {
        case: "the configuration names only the endpoint to notify",
        topic_named_by_arn: false,
        endpoint_configured: true,
        topic_name: None,
        delivered: true,
    },
    Row {
        case: "the configuration names the endpoint and the topic's name",
        topic_named_by_arn: false,
        endpoint_configured: true,
        topic_name: Some("environment-events-topic"),
        delivered: true,
    },
    Row {
        case: "the configuration names no notification settings at all",
        topic_named_by_arn: false,
        endpoint_configured: false,
        topic_name: None,
        delivered: false,
    },
];

/// Every configured environment has BOTH kinds of event published to its topic —
/// the one its create recorded and the one the clock made due — and an
/// environment configured with none has nothing published for it.
#[tokio::test]
async fn an_environments_events_are_published_to_the_topic_its_configuration_names() {
    // The two events a launch records, through the two paths an event is
    // recorded by: the call itself, and the clock that completes the launch.
    let started = "createEnvironment is starting.";
    let completed = format!("Successfully launched environment: {ENVIRONMENT}");
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_notification_services();
        let (queue_url, queue_arn) = given_queue(&emulator).await;
        given_application(&emulator).await;

        let mut settings: Vec<(&str, String)> = Vec::new();
        if row.topic_named_by_arn {
            // The caller's own topic, with the queue already subscribed to it.
            let created = sns(&emulator, "CreateTopic", &[("Name", QUEUE)]).await;
            let topic_arn = created
                .expect("CreateTopicResult/TopicArn", case)
                .to_string();
            sns(
                &emulator,
                "Subscribe",
                &[
                    ("TopicArn", &topic_arn),
                    ("Protocol", "sqs"),
                    ("Endpoint", &queue_arn),
                ],
            )
            .await;
            settings.push((eb::state::OPTION_NOTIFICATION_TOPIC_ARN, topic_arn));
        }
        if row.endpoint_configured {
            settings.push((eb::state::OPTION_NOTIFICATION_ENDPOINT, queue_arn.clone()));
            settings.push((eb::state::OPTION_NOTIFICATION_PROTOCOL, "sqs".to_string()));
        }
        if let Some(topic_name) = row.topic_name {
            settings.push((
                eb::state::OPTION_NOTIFICATION_TOPIC_NAME,
                topic_name.to_string(),
            ));
        }
        let written: Vec<(&str, &str)> = settings
            .iter()
            .map(|(option, value)| (*option, value.as_str()))
            .collect();
        create_environment(&emulator, case, &written).await;

        std::thread::sleep(PAST_DUE);
        let envelopes = await_delivery(&emulator, &queue_url, &[started, &completed]).await;

        if !row.delivered {
            assert!(
                envelopes.is_empty(),
                "case {case:?}: an environment configured with no notification settings must \
                 have nothing published for it, got {envelopes:?}"
            );
            continue;
        }
        for want in [started, completed.as_str()] {
            assert!(
                envelopes.iter().any(|envelope| carries(envelope, want)),
                "case {case:?}: expected a notification carrying {want:?}, got {envelopes:?}"
            );
        }
        // A topic Elastic Beanstalk created for the configured endpoint is
        // reported by the environment's own configuration, under the name the
        // configuration asked for or the service's own name for it.
        if !row.topic_named_by_arn {
            let expected_name = row.topic_name.map(str::to_string).unwrap_or_else(|| {
                format!("ElasticBeanstalkNotifications-Environment-{ENVIRONMENT}")
            });
            let reported = reported_topic_arn(&emulator, case).await;
            assert!(
                reported.ends_with(&format!(":{expected_name}")),
                "case {case:?}: expected the configuration to report a topic named \
                 {expected_name:?}, got {reported:?}"
            );
        }
    }
}

/// What one notification carries: the subject line names the service, and the
/// body reports the event with the resources it is about.
#[tokio::test]
async fn a_notification_reports_the_event_and_the_environment_it_is_about() {
    let case = "a launched environment's first notification";
    let emulator = Emulator::with_notification_services();
    let (queue_url, queue_arn) = given_queue(&emulator).await;
    given_application(&emulator).await;
    create_environment(
        &emulator,
        case,
        &[
            (eb::state::OPTION_NOTIFICATION_ENDPOINT, &queue_arn),
            (eb::state::OPTION_NOTIFICATION_PROTOCOL, "sqs"),
        ],
    )
    .await;

    let envelopes = await_delivery(&emulator, &queue_url, &["createEnvironment is starting."]).await;
    let notification = envelopes
        .iter()
        .find(|envelope| carries(envelope, "createEnvironment is starting."))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the create's own notification, got {envelopes:?}")
        });
    let subject = notification
        .get("Subject")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        subject.starts_with("AWS Elastic Beanstalk Notification - "),
        "case {case:?}: expected the subject to name the service, got {subject:?}"
    );
    let message = notification
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for expected in [
        "Message: createEnvironment is starting.",
        &format!("Environment: {ENVIRONMENT}"),
        &format!("Application: {APPLICATION}"),
        "Timestamp: ",
    ] {
        assert!(
            message.contains(expected),
            "case {case:?}: expected {expected:?} in the notification body, got {message:?}"
        );
    }
}
