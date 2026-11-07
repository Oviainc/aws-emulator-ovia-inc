// SPDX-License-Identifier: CC0-1.0

//! What a `lambda`-protocol delivery COMES TO in the subscribed function: SNS, Lambda and
//! Amazon SQS on ONE engine, a real `python3.12` package whose handler forwards the event it
//! was handed into its own queue, and the assertion made on that queue — so an invocation is
//! observed through the effect the handler itself had, never through a record this emulator
//! keeps of having tried.
//!
//! The RULE asserted here: a confirmed subscription whose protocol is `lambda` ("delivery of
//! JSON-encoded message to an Lambda function", `SubscribeInput$Protocol`) has its function
//! invoked once per published message — SNS "delivers the message to each endpoint that is
//! subscribed to the topic" (`Publish`) — and ONLY where that function's own resource-based
//! policy grants this service `lambda:InvokeFunction` on it. A delivery the policy withholds
//! reaches nothing, and the publish answers its modelled 200 with a `MessageId` either way.
//!
//! The rows of the first table differ along what the rule's authorization half generalises
//! over: whether the function carries a policy at all, which action it grants, which principal
//! it grants it to, and whether the grant's `SourceArn` condition names the publishing topic,
//! another one, or a wildcard the publishing topic matches. The rows of the second differ along
//! the form the message was published in, which is what decides the body the function receives.
//!
//! The event's own members are pinned in `lambda_subscription_delivery.rs`, which reads them
//! where Lambda holds them; this suite is about the run that follows.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::sqs_subscription_delivery::element;
use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound, and the prefix a function's `AddPermission` is bound under.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the Lambda model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute every deployment package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The service principal a subscribed function's policy has to grant for an SNS notification
/// to reach it, and the action it has to grant on it.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// How long this suite watches a function's sink queue for the event its handler forwarded:
/// the invocation is asynchronous, so the run happens off the publish's response path.
const WATCH_BUDGET: Duration = Duration::from_secs(40);

/// How long to look again at a sink that is still empty.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// How long a delivery that must NOT happen is given after every delivery that must: the
/// withheld legs of one publish are decided in the same fan-out as the granted ones, so once
/// the granted ones have arrived and this has passed, a sink still empty was never written to.
const SETTLE: Duration = Duration::from_secs(2);

/// SNS, Lambda and Amazon SQS on ONE engine, as `core/router/src/lib.rs` mounts them: the
/// handler's own SDK call is dispatched back into this same router, so the queue it writes to
/// is the queue this suite reads.
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
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. Neither the key id nor the signature is a credential: request authentication
/// is off in this harness.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery form call against SNS that must SUCCEED, as its body text.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "{action}: expected HTTP 200, got {status} / {text}"
    );
    text
}

/// One `awsJson1_0` call against Amazon SQS that must SUCCEED.
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
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One `restJson1` call against Lambda, with whatever status it answered.
async fn lambda(emulator: &Emulator, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
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

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler every function below runs: it forwards the WHOLE event it was handed into its
/// own queue, so what this suite reads is what the handler was invoked with — and a sink that
/// stays empty is a function that never ran.
const FORWARDING_HANDLER: &str = r#"
import json
import os

import boto3


SINK = os.environ["SINK_QUEUE_URL"]


def lambda_handler(event, context):
    boto3.client("sqs").send_message(QueueUrl=SINK, MessageBody=json.dumps(event))
    return {"forwarded": len(event["Records"])}
"#;

/// A queue the handlers forward into: its url.
async fn given_sink(emulator: &Emulator, name: &str) -> String {
    let created = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string()
}

/// Create a function whose handler forwards its event into the queue at `sink`: the ARN it
/// reports, which is the ARN a `lambda` subscription names it by.
async fn given_function(emulator: &Emulator, case: &str, name: &str, sink: &str) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        FUNCTIONS_URI,
        json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Environment": { "Variables": { "SINK_QUEUE_URL": sink } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(FORWARDING_HANDLER)),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {created}"))
        .to_string()
}

/// `AddPermission` on the function `name`, with the statement `permission` describes.
async fn grant(emulator: &Emulator, case: &str, name: &str, permission: &Value) {
    let (status, answered) = lambda(
        emulator,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/policy"),
        permission.clone(),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission, got {status} ({answered})"
    );
}

/// Subscribe the function `function_arn` names to `topic` over the `lambda` protocol: the
/// subscription's ARN.
async fn given_subscription(emulator: &Emulator, topic: &str, function_arn: &str) -> String {
    let answered = sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", topic),
            ("Protocol", "lambda"),
            ("Endpoint", function_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    element(&answered, "SubscriptionArn")
}

/// Every message the queue at `url` holds right now, as the JSON its body carries.
async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    loop {
        let received = sqs(
            emulator,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                // Long enough that a message counted here is not handed over again while
                // this loop reads the rest.
                "VisibilityTimeout": 300,
            }),
        )
        .await;
        let handed = received["Messages"].as_array().cloned().unwrap_or_default();
        if handed.is_empty() {
            return held;
        }
        for message in handed {
            let body = message["Body"].as_str().unwrap_or_default().to_string();
            held.push(serde_json::from_str(&body).unwrap_or(Value::String(body)));
        }
    }
}

/// Watch the queue at `url` until its handler has forwarded `wanted` events, answering them.
/// Panics when the budget runs out first, naming what did arrive.
async fn forwarded_events(emulator: &Emulator, case: &str, url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        seen.extend(drain(emulator, url).await);
        if seen.len() >= wanted {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {wanted} forwarded event(s) within {WATCH_BUDGET:?}, got \
             {seen:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row of the authorization table: the policy the subscribed function carries, and whether
/// the publish must reach its handler.
struct Row {
    case: &'static str,
    /// The `AddPermission` request the function is granted with, or `None` for a function that
    /// carries no resource-based policy at all.
    permission: Option<Value>,
    /// How many times the published message must reach the function's handler.
    invocations: usize,
}

/// The rows, built against the topic that will publish (`topic`) and one that will not
/// (`other_topic`), because a grant's `SourceArn` condition names a topic.
fn rows(topic: &str, other_topic: &str) -> Vec<Row> {
    vec![
        Row {
            case: "a grant to this service naming no source",
            permission: Some(json!({
                "StatementId": "sns-invoke-unscoped",
                "Action": INVOKE_ACTION,
                "Principal": SNS_PRINCIPAL,
            })),
            invocations: 1,
        },
        Row {
            // The grant a high-level subscription construct adds: scoped to the topic that
            // publishes.
            case: "a grant scoped to the publishing topic",
            permission: Some(json!({
                "StatementId": "sns-invoke-this-topic",
                "Action": INVOKE_ACTION,
                "Principal": SNS_PRINCIPAL,
                "SourceArn": topic,
            })),
            invocations: 1,
        },
        Row {
            case: "a grant scoped to another topic",
            permission: Some(json!({
                "StatementId": "sns-invoke-other-topic",
                "Action": INVOKE_ACTION,
                "Principal": SNS_PRINCIPAL,
                "SourceArn": other_topic,
            })),
            invocations: 0,
        },
        Row {
            // The bare subscription a low-level template declares: no permission at all.
            case: "no resource-based policy at all",
            permission: None,
            invocations: 0,
        },
        Row {
            case: "a grant of another action on the function",
            permission: Some(json!({
                "StatementId": "sns-get-function",
                "Action": "lambda:GetFunction",
                "Principal": SNS_PRINCIPAL,
            })),
            invocations: 0,
        },
        Row {
            case: "a grant of the invocation to another service",
            permission: Some(json!({
                "StatementId": "s3-invoke",
                "Action": INVOKE_ACTION,
                "Principal": "s3.amazonaws.com",
            })),
            invocations: 0,
        },
        Row {
            // A wildcard source the publishing topic's ARN matches: the grant is scoped, and
            // this publish is inside the scope. The account is spelled out because the model's
            // `SourceArn` pattern admits only a 12-digit account or none.
            case: "a grant scoped to a wildcard the topic matches",
            permission: Some(json!({
                "StatementId": "sns-invoke-wildcard",
                "Action": INVOKE_ACTION,
                "Principal": SNS_PRINCIPAL,
                "SourceArn": format!(
                    "{}:*",
                    topic.rsplit_once(':').map_or(topic, |(prefix, _)| prefix),
                ),
            })),
            invocations: 1,
        },
    ]
}

/// RULE: a published message runs the handler of every function subscribed to the topic whose
/// own resource-based policy grants this service `lambda:InvokeFunction` on it, and of no other
/// — whatever the publish answers, which is its modelled 200 with a `MessageId` either way.
#[tokio::test]
async fn a_published_message_runs_the_handler_of_every_function_its_policy_admits() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "orders")]).await,
        "TopicArn",
    );
    let other_topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "invoices")]).await,
        "TopicArn",
    );
    let rows = rows(&topic, &other_topic);

    // Every row's function, subscribed to the one topic that publishes, each forwarding into
    // its own sink: one publish then fans out to all of them.
    let mut sinks: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("grant-subscriber-{index}");
        let sink = given_sink(&emulator, &format!("grant-sink-{index}")).await;
        let function_arn = given_function(&emulator, row.case, &name, &sink).await;
        if let Some(permission) = &row.permission {
            grant(&emulator, row.case, &name, permission).await;
        }
        let subscription = given_subscription(&emulator, &topic, &function_arn).await;
        assert!(
            subscription.starts_with(&topic),
            "case {:?}: a lambda subscription is confirmed at once and named under its topic, \
             got {subscription:?}",
            row.case
        );
        sinks.push(sink);
    }
    // The functions reach `Active` on the clock, not on a call: nothing here invokes one
    // before they do.
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(250)).await;

    let marker = "marker-fan-out";
    let published = sns(
        &emulator,
        "Publish",
        &[("TopicArn", &topic), ("Message", marker)],
    )
    .await;
    let message_id = element(&published, "MessageId");
    assert!(
        !message_id.is_empty(),
        "the publish must answer a MessageId whether or not a subscriber could be invoked, got \
         {published}"
    );

    // Wait for every delivery that MUST happen, then give the rest of the fan-out the same
    // room: a sink still empty after that was never written to, and a granted sink holding
    // more than one event was invoked more often than the publish owed.
    let mut observed: Vec<Vec<Value>> = Vec::new();
    for (row, sink) in rows.iter().zip(&sinks) {
        observed.push(if row.invocations > 0 {
            forwarded_events(&emulator, row.case, sink, row.invocations).await
        } else {
            Vec::new()
        });
    }
    tokio::time::sleep(SETTLE).await;

    for ((row, sink), events) in rows.iter().zip(&sinks).zip(observed.iter_mut()) {
        events.extend(drain(&emulator, sink).await);
        assert_eq!(
            events.len(),
            row.invocations,
            "case {:?}: expected {} run(s) of the subscribed function's handler, got {events:?}",
            row.case,
            row.invocations
        );
        let Some(event) = events.first() else {
            continue;
        };
        let records = event["Records"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            records.len(),
            1,
            "case {:?}: one publish is one notification, so the event carries one record, got \
             {event}",
            row.case
        );
        assert_eq!(
            records[0]["Sns"]["Message"].as_str(),
            Some(marker),
            "case {:?}: the handler must be handed the message as published, got {event}",
            row.case
        );
        assert_eq!(
            records[0]["Sns"]["MessageId"].as_str(),
            Some(message_id.as_str()),
            "case {:?}: the handler must be handed the id the publish answered, got {event}",
            row.case
        );
    }
}

/// RULE: the `Message` a subscribed function receives is the branch a structured message names
/// for its own protocol — "Set MessageStructure to json if you want to send a different message
/// for each protocol" (`PublishInput$MessageStructure`) — falling back to the `default` branch,
/// and the whole published message when it carries no structure at all. Each publish runs the
/// handler once more.
#[tokio::test]
async fn a_function_receives_the_branch_a_structured_message_names_for_it() {
    // (case, MessageStructure, the message as published, the body the handler must receive)
    let rows: &[(&str, Option<&str>, &str, &str)] = &[
        (
            "no structure at all",
            None,
            r#"{"default":"not a structure here"}"#,
            r#"{"default":"not a structure here"}"#,
        ),
        (
            "a structure naming this protocol",
            Some("json"),
            r#"{"default":"for everyone","lambda":"for the function","sqs":"for the queue"}"#,
            "for the function",
        ),
        (
            "a structure naming only the default",
            Some("json"),
            r#"{"default":"for everyone"}"#,
            "for everyone",
        ),
    ];

    let case = "a granted function subscribed to the topic";
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "branches")]).await,
        "TopicArn",
    );
    let sink = given_sink(&emulator, "branches-sink").await;
    let function_arn = given_function(&emulator, case, "branch-consumer", &sink).await;
    grant(
        &emulator,
        case,
        "branch-consumer",
        &json!({
            "StatementId": "sns-invoke",
            "Action": INVOKE_ACTION,
            "Principal": SNS_PRINCIPAL,
            "SourceArn": topic,
        }),
    )
    .await;
    given_subscription(&emulator, &topic, &function_arn).await;
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(250)).await;

    for (case, structure, message, expected) in rows {
        let mut params: Vec<(&str, &str)> = vec![("TopicArn", &topic), ("Message", message)];
        if let Some(structure) = structure {
            params.push(("MessageStructure", structure));
        }
        sns(&emulator, "Publish", &params).await;
        let events = forwarded_events(&emulator, case, &sink, 1).await;
        let delivered = events[0]["Records"][0]["Sns"]["Message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            delivered, *expected,
            "case {case:?}: expected the handler to receive {expected:?}, got {delivered:?}"
        );
    }
}
