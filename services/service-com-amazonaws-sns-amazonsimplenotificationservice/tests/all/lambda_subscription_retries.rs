// SPDX-License-Identifier: CC0-1.0

//! What happens to a `lambda`-protocol delivery the subscribed function could NOT take: it is
//! RETRIED while the function cannot take it yet, and PARKED in the subscription's dead-letter
//! queue when the function will not take it at all. End to end over the wire through the real
//! `MultiServiceRouter`, over the harness `lambda_subscription_delivery.rs` builds (SNS,
//! Lambda and Amazon SQS on one engine), so there is one wire and one fixture for this
//! surface rather than a second copy.
//!
//! The RULES asserted here, both about one delivery of one published message:
//!
//! * "Amazon SNS defines a delivery policy for each delivery protocol. The delivery policy
//!   defines how Amazon SNS retries the delivery of messages when server-side errors occur" —
//!   so a delivery the endpoint could not take YET is retried rather than dropped. A function
//!   inside the activation window its own `State` reports is exactly that endpoint: "During
//!   this time, you can't invoke or modify the function" (`CreateFunction`), and the
//!   notification published inside that window still reaches it.
//! * "RedrivePolicy – When specified, sends undeliverable messages to the specified Amazon SQS
//!   dead-letter queue. Messages that can't be delivered due to client errors (for example,
//!   when the subscribed endpoint is unreachable) […] are held in the dead-letter queue for
//!   further analysis or reprocessing" (`SubscribeInput$Attributes`) — so a delivery the
//!   function's own resource policy withholds is held in that queue as the undeliverable
//!   MESSAGE, the notification every enveloped subscriber of the topic receives, and the
//!   function is not invoked at all. The `Records` array is not part of that message: it is the
//!   envelope an INVOCATION carries its event in, and in this row no invocation was made.
//!
//! The rows differ along what the rules generalise over: whether the endpoint can take the
//! delivery yet, whether it will take it at all, whether the subscription asks for a redrive,
//! and how many messages are published before the window passes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::lambda_subscription_delivery::{
    emulator, given_function, grant, invoked, lambda, publish, quiet, sns, sqs, subscribe,
    Emulator, INVOKE_ACTION, LAMBDA_PROTOCOL, ROLE, SNS_PRINCIPAL, ZIP_FILE,
};
use crate::sqs_subscription_delivery::element;

/// How long a wait for the dead-letter copy lasts before giving up. The park happens on the
/// publishing request, so this is many deliveries' worth of slack rather than a span the
/// behaviour depends on.
const PARK_BUDGET: Duration = Duration::from_secs(10);

/// How often a wait looks again.
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// A function called `name` that is NOT brought forward to `Active`: it stays `Pending` for
/// the activation window its own `State` reports, so a notification published now is one the
/// function cannot take yet. Its ARN.
async fn given_pending_function(emulator: &Emulator, name: &str) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": ZIP_FILE },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction {name}: expected HTTP 201, got {status} with {created}"
    );
    let state = lambda(
        emulator,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(
        state["State"].as_str(),
        Some("Pending"),
        "a function is Pending for its activation window, got {state}"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction {name}: no FunctionArn in {created}"))
        .to_string()
}

/// A queue called `name` that Amazon SNS may send to: its url and the ARN a `RedrivePolicy`
/// names it by.
async fn given_dead_letter_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let created = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string();
    // The parked copy is sent by SNS itself, so it is the QUEUE's own policy that has to grant
    // the send: "you must ensure that the Amazon SQS queue policy allows the Amazon SNS topic
    // to send messages to the queue".
    sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {
                "Policy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Sid": "AllowSNSDelivery",
                        "Effect": "Allow",
                        "Principal": { "Service": SNS_PRINCIPAL },
                        "Action": "sqs:SendMessage",
                        "Resource": arn,
                    }],
                })
                .to_string(),
            },
        }),
    )
    .await;
    (url, arn)
}

/// Subscribe the function `function_arn` names to `topic` with a `RedrivePolicy` naming
/// `dead_letter_arn`: the subscription's ARN.
async fn given_redriven_subscription(
    emulator: &Emulator,
    topic: &str,
    function_arn: &str,
    dead_letter_arn: &str,
) -> String {
    let redrive = json!({ "deadLetterTargetArn": dead_letter_arn }).to_string();
    let answered = sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", topic),
            ("Protocol", LAMBDA_PROTOCOL),
            ("Endpoint", function_arn),
            ("ReturnSubscriptionArn", "true"),
            ("Attributes.entry.1.key", "RedrivePolicy"),
            ("Attributes.entry.1.value", &redrive),
        ],
    )
    .await;
    element(&answered, "SubscriptionArn")
}

/// Everything the queue at `url` holds once it holds at least one message — or everything it
/// holds at the deadline, which is what a delivery that was never parked leaves.
async fn parked(emulator: &Emulator, url: &str) -> Vec<String> {
    let deadline = Instant::now() + PARK_BUDGET;
    loop {
        let received = sqs(
            emulator,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let held: Vec<String> = received["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect();
        if !held.is_empty() || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// RULE: a delivery the subscribed function could not take YET is retried along the phases
/// SNS's own delivery policy publishes, so a notification published while the function is
/// still inside its activation window reaches that function once the window passes.
///
/// The rows differ in how many messages are published inside the window: the rule is per
/// message, so each of them is delivered.
#[tokio::test]
async fn a_delivery_the_function_cannot_take_yet_is_retried_until_it_can() {
    // (case, how many messages are published inside the activation window)
    let rows: &[(&str, usize)] = &[
        ("one message published inside the activation window", 1),
        ("three messages published inside the activation window", 3),
    ];

    for (index, (case, published)) in rows.iter().enumerate() {
        let emulator = emulator();
        let function = format!("pending-subscriber-{index}");
        // NOT brought forward: the function is `Pending` when the publish below fans out, so
        // the invocation SNS makes is one AWS Lambda refuses at this time.
        let function_arn = given_pending_function(&emulator, &function).await;
        let topic = element(
            &sns(
                &emulator,
                "CreateTopic",
                &[("Name", &format!("pending-topic-{index}"))],
            )
            .await,
            "TopicArn",
        );
        subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;
        grant(
            &emulator,
            &function,
            "sns-invokes",
            INVOKE_ACTION,
            SNS_PRINCIPAL,
            Some(&topic),
        )
        .await;

        let mut markers: Vec<String> = Vec::new();
        for round in 0..*published {
            let marker = format!("{function}-marker-{round}");
            let message_id = publish(&emulator, &topic, &[("Message", &marker)]).await;
            assert!(
                !message_id.is_empty(),
                "case {case:?}: the publish is answered with a MessageId whether or not the \
                 function could be invoked, got {message_id:?}"
            );
            markers.push(marker);
        }

        let events = invoked(&emulator, &function, *published).await;
        assert_eq!(
            events.len(),
            *published,
            "case {case:?}: every notification published inside the window must reach the \
             function once it can take the invocation, got {events:?}"
        );
        let mut delivered: Vec<String> = events
            .iter()
            .map(|event| {
                event["Records"][0]["Sns"]["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        delivered.sort();
        markers.sort();
        assert_eq!(
            delivered, markers,
            "case {case:?}: the function must be handed each published message exactly once, \
             got {delivered:?}"
        );
    }
}

/// RULE: a delivery the subscribed function WILL NOT take — its own resource policy grants
/// this service nothing — is held in the dead-letter queue the subscription's `RedrivePolicy`
/// names, as the undeliverable MESSAGE rather than as the event envelope an invocation would
/// have carried; the function is not invoked at all, and once the grant is added the next
/// publish reaches it with nothing parked.
#[tokio::test]
async fn a_delivery_the_function_will_not_take_is_held_in_the_dead_letter_queue() {
    let case = "a function whose policy grants nothing, under a RedrivePolicy";
    let emulator = emulator();
    let (dead_letter_url, dead_letter_arn) =
        given_dead_letter_queue(&emulator, "withheld-notifications").await;
    let function = "withheld-subscriber";
    let function_arn = given_function(&emulator, function).await;
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "withheld-topic")]).await,
        "TopicArn",
    );
    let subscription =
        given_redriven_subscription(&emulator, &topic, &function_arn, &dead_letter_arn).await;

    publish(&emulator, &topic, &[("Message", "withheld")]).await;

    let held = parked(&emulator, &dead_letter_url).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: the notification that could not be delivered must be held in the \
         dead-letter queue, got {held:?}"
    );
    let notification: Value = serde_json::from_str(&held[0]).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the parked copy must be the notification that could not be \
             delivered ({error}): {}",
            held[0]
        )
    });
    assert!(
        notification.get("Records").is_none(),
        "case {case:?}: the parked copy is the undeliverable message, not the event envelope an \
         invocation carries, got {notification}"
    );
    assert_eq!(
        notification["Type"].as_str(),
        Some("Notification"),
        "case {case:?}: the parked copy is the notification of the publish, got {notification}"
    );
    assert_eq!(
        notification["Message"].as_str(),
        Some("withheld"),
        "case {case:?}: the parked copy carries the message as published, got {notification}"
    );
    assert_eq!(
        notification["TopicArn"].as_str(),
        Some(topic.as_str()),
        "case {case:?}: the parked copy names the topic published to, got {notification}"
    );
    assert!(
        notification["UnsubscribeURL"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("SubscriptionArn={subscription}")),
        "case {case:?}: the parked copy names the subscription that could not receive it, got \
         {notification}"
    );
    let events = quiet(&emulator, function).await;
    assert!(
        events.is_empty(),
        "case {case:?}: the function must not be invoked at all, got {events:?}"
    );

    // The client adds the grant: the NEXT publish reaches the function, and nothing more is
    // parked.
    grant(
        &emulator,
        function,
        "sns-invokes",
        INVOKE_ACTION,
        SNS_PRINCIPAL,
        Some(&topic),
    )
    .await;
    publish(&emulator, &topic, &[("Message", "granted")]).await;
    let events = invoked(&emulator, function, 1).await;
    assert_eq!(
        events.len(),
        1,
        "case \"the grant added\": the function must be invoked once, got {events:?}"
    );
    assert_eq!(
        events[0]["Records"][0]["Sns"]["Message"].as_str(),
        Some("granted"),
        "case \"the grant added\": the function must receive the message just published, got \
         {:?}",
        events[0]
    );
    let received = sqs(
        &emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": dead_letter_url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    let still_parked = received["Messages"].as_array().cloned().unwrap_or_default();
    assert!(
        still_parked.is_empty(),
        "case \"the grant added\": a delivered notification parks nothing, got {received}"
    );
}
