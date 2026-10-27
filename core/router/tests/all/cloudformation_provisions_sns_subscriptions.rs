// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::SNS::Subscription` resources its template declares — in Amazon SNS.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns it created it, so the handle the stack publishes for that resource
//! (`Ref`/`Fn::GetAtt`) addresses a resource that service really holds.** A subscription is
//! a resource of Amazon SNS's own: the handle is the subscription ARN `Subscribe` mints, so
//! after the create waiter returns the topic really has that subscriber —
//! `ListSubscriptionsByTopic` reports it, `GetSubscriptionAttributes` on the published ARN
//! answers with the attributes the template declared, and a `Publish` to the topic is
//! delivered to the subscribed queue. Three crates have to agree for that to hold
//! (CloudFormation makes the calls, SNS serves them and performs the delivery, SQS receives
//! it), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: how many subscriptions one stack
//! declares, whether a subscription declares attributes at all, how the template WRITES an
//! attribute value (a JSON boolean or the string Amazon SNS carries it as), whether the
//! declaration names the topic's `Region` explicitly, and therefore which form the
//! subscriber receives the published message in (the notification envelope, or the bare body
//! a raw-delivery subscriber takes).
//!
//! The teardown half is a row of the same rule: a revision that stops declaring the
//! subscription UNSUBSCRIBES it, so the topic reports no subscriber and the next publish
//! reaches nothing.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the stack's topic, queues
/// and their ARNs belong to. Deliberately NOT the emulator's default region: a stack's
/// resources carry the Region the stack was created in.
const REGION: &str = "us-west-2";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition (500 ms) to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The body every row publishes, so a received message is recognised by its content.
const MESSAGE: &str = "a stack-declared subscriber";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` value the way an AWS SDK does, so
/// a template body full of `{`, `"` and `&` survives the body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One awsQuery call of `signing_name`.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation operation.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Amazon SNS operation that must be answered.
async fn sns_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, String)],
) -> String {
    let (status, body) = query(router, "sns", "2010-03-31", action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {body}"
    );
    body
}

/// One Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    found
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> std::collections::BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// A `GetSubscriptionAttributes` answer's attribute map: each `<entry>`'s `<key>` paired
/// with its own `<value>`.
fn attributes_of(xml: &str) -> std::collections::BTreeMap<String, String> {
    xml_texts(xml, "key")
        .into_iter()
        .zip(xml_texts(xml, "value"))
        .collect()
}

/// The ARN of the topic called `name`, as this emulator mints it.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

/// The ARN of the queue called `name`.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// The url of the queue called `name`.
fn queue_url(name: &str) -> String {
    format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/{name}")
}

/// The queue policy an SNS subscriber needs, declared in the template itself: the service
/// principal `sns.amazonaws.com` allowed `sqs:SendMessage` on the queues the stack creates,
/// written as the intrinsic that resolves to each queue's own ARN.
fn grant_to_sns(queues: &[&str]) -> Value {
    let resources: Vec<Value> = queues
        .iter()
        .map(|queue| json!({"Fn::GetAtt": [queue, "Arn"]}))
        .collect();
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowTheStacksTopic",
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": resources,
        }],
    })
}

/// What one row declares about one subscriber.
struct DeclaredSubscription {
    /// The resource's logical id, which is also the output key its `Ref` is published under.
    logical_id: &'static str,
    /// The logical id of the queue it subscribes, and that queue's name.
    queue_logical_id: &'static str,
    queue_name: &'static str,
    /// The `Properties` the template declares beside `TopicArn`, `Protocol` and `Endpoint`.
    declared_properties: fn() -> Value,
    /// The attributes `GetSubscriptionAttributes` must then report for it, computed from
    /// what the row declares rather than read back out of the answer being checked.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// Whether the subscriber receives the bare published body rather than the notification
    /// envelope — which is what `RawMessageDelivery` asks for.
    raw: bool,
}

/// One row: a stack, the topic it declares and the subscribers on it.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    topic_name: &'static str,
    subscriptions: &'static [DeclaredSubscription],
}

const ROWS: &[Row] = &[
    Row {
        case: "one subscriber declaring no attributes",
        stack_name: "one-subscriber-stack",
        topic_name: "one-subscriber-topic",
        subscriptions: &[DeclaredSubscription {
            logical_id: "Plain",
            queue_logical_id: "PlainQueue",
            queue_name: "one-subscriber-queue",
            declared_properties: || json!({}),
            expected_attributes: &[],
            raw: false,
        }],
    },
    Row {
        case: "two subscribers, one asking for raw delivery as a JSON boolean",
        stack_name: "two-subscriber-stack",
        topic_name: "two-subscriber-topic",
        subscriptions: &[
            DeclaredSubscription {
                logical_id: "Enveloped",
                queue_logical_id: "EnvelopedQueue",
                queue_name: "two-subscriber-enveloped-queue",
                declared_properties: || json!({}),
                expected_attributes: &[],
                raw: false,
            },
            DeclaredSubscription {
                logical_id: "Raw",
                queue_logical_id: "RawQueue",
                queue_name: "two-subscriber-raw-queue",
                declared_properties: || json!({"RawMessageDelivery": true}),
                expected_attributes: &[("RawMessageDelivery", "true")],
                raw: true,
            },
        ],
    },
    Row {
        case: "one subscriber naming its topic's Region, with raw delivery written as text",
        stack_name: "regional-subscriber-stack",
        topic_name: "regional-subscriber-topic",
        subscriptions: &[DeclaredSubscription {
            logical_id: "Regional",
            queue_logical_id: "RegionalQueue",
            queue_name: "regional-subscriber-queue",
            declared_properties: || json!({"RawMessageDelivery": "true", "Region": REGION}),
            expected_attributes: &[("RawMessageDelivery", "true")],
            raw: true,
        }],
    },
];

/// The template one row declares: the topic, one queue per subscriber, the queue policy that
/// lets the topic send to them, and the subscriptions themselves — with every reference
/// written as the intrinsic a client would write.
fn template_of(row: &Row) -> Value {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "Topic".to_string(),
        json!({"Type": "AWS::SNS::Topic", "Properties": {"TopicName": row.topic_name}}),
    );
    let queues: Vec<&str> = row
        .subscriptions
        .iter()
        .map(|subscription| subscription.queue_logical_id)
        .collect();
    let mut queue_refs = Vec::new();
    for subscription in row.subscriptions {
        resources.insert(
            subscription.queue_logical_id.to_string(),
            json!({
                "Type": "AWS::SQS::Queue",
                "Properties": {"QueueName": subscription.queue_name},
            }),
        );
        queue_refs.push(json!({"Ref": subscription.queue_logical_id}));
    }
    resources.insert(
        "Grant".to_string(),
        json!({
            "Type": "AWS::SQS::QueuePolicy",
            "Properties": {"Queues": queue_refs, "PolicyDocument": grant_to_sns(&queues)},
        }),
    );
    let mut outputs = serde_json::Map::new();
    outputs.insert("TopicArn".to_string(), json!({"Value": {"Ref": "Topic"}}));
    for subscription in row.subscriptions {
        let mut properties = json!({
            "TopicArn": {"Ref": "Topic"},
            "Protocol": "sqs",
            "Endpoint": {"Fn::GetAtt": [subscription.queue_logical_id, "Arn"]},
        });
        let declared = (subscription.declared_properties)();
        if let (Some(target), Some(declared)) = (properties.as_object_mut(), declared.as_object())
        {
            for (name, value) in declared {
                target.insert(name.clone(), value.clone());
            }
        }
        resources.insert(
            subscription.logical_id.to_string(),
            json!({"Type": "AWS::SNS::Subscription", "Properties": properties}),
        );
        outputs.insert(
            subscription.logical_id.to_string(),
            json!({"Value": {"Ref": subscription.logical_id}}),
        );
    }
    json!({"Resources": Value::Object(resources), "Outputs": Value::Object(outputs)})
}

/// Create a stack, wait past its transition, and answer its `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &Value,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack_name.to_string()),
            ("TemplateBody", template.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name.to_string())]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    body
}

/// Every body waiting on the queue at `url`, as the text each message carries.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A stack's `AWS::SNS::Subscription` resources are subscriptions Amazon SNS really holds:
/// the ARN the stack publishes is theirs, the topic lists them, their declared attributes
/// read back, and a publish to the topic is delivered to each subscribed queue in the form
/// that subscription asked for.
#[tokio::test]
async fn a_stack_declared_subscription_is_a_subscriber_the_topic_really_has() {
    for row in ROWS {
        let router = build_router();
        let template = template_of(row);
        let described = create_and_settle(&router, row.case, row.stack_name, &template).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: the stack must have created every resource it declares, got {described}",
            row.case
        );

        let outputs = outputs_of(&described);
        let topic = topic_arn(row.topic_name);
        assert_eq!(
            outputs.get("TopicArn").map(String::as_str),
            Some(topic.as_str()),
            "case {:?}: the stack must publish the topic's own ARN, got {:?}",
            row.case,
            outputs.get("TopicArn")
        );

        // The handle the stack publishes for each subscription is the ARN Amazon SNS minted
        // for it: the topic's ARN plus the id segment SNS appends per subscription.
        let mut published: Vec<String> = Vec::new();
        for subscription in row.subscriptions {
            let arn = outputs
                .get(subscription.logical_id)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the stack published no handle for {}: {described}",
                        row.case, subscription.logical_id
                    )
                })
                .clone();
            let id = arn.strip_prefix(&format!("{topic}:")).unwrap_or_default();
            assert!(
                !id.is_empty() && !id.contains(':'),
                "case {:?}: {} must publish a subscription ARN of its own topic, got {arn:?}",
                row.case,
                subscription.logical_id
            );
            published.push(arn);
        }

        // The topic lists exactly those subscriptions, each naming the queue it was
        // declared against.
        let listed = sns_ok(
            &router,
            row.case,
            "ListSubscriptionsByTopic",
            &[("TopicArn", topic.clone())],
        )
        .await;
        let mut reported: Vec<String> = xml_texts(&listed, "SubscriptionArn");
        reported.sort();
        let mut expected = published.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {:?}: the topic must list the subscriptions the stack created, got {listed}",
            row.case
        );
        let endpoints: Vec<String> = xml_texts(&listed, "Endpoint");
        for subscription in row.subscriptions {
            assert!(
                endpoints.contains(&queue_arn(subscription.queue_name)),
                "case {:?}: the listing must name {}'s queue, got {endpoints:?}",
                row.case,
                subscription.logical_id
            );
        }

        // Each subscription reports the attributes its declaration asked for.
        for (subscription, arn) in row.subscriptions.iter().zip(&published) {
            let read = sns_ok(
                &router,
                row.case,
                "GetSubscriptionAttributes",
                &[("SubscriptionArn", arn.clone())],
            )
            .await;
            let attributes = attributes_of(&read);
            assert_eq!(
                attributes.get("TopicArn").map(String::as_str),
                Some(topic.as_str()),
                "case {:?}: {} must belong to the stack's topic, got {read}",
                row.case,
                subscription.logical_id
            );
            for (name, value) in subscription.expected_attributes {
                assert_eq!(
                    attributes.get(*name).map(String::as_str),
                    Some(*value),
                    "case {:?}: {} must report the declared {name}, got {:?}",
                    row.case,
                    subscription.logical_id,
                    attributes.get(*name)
                );
            }
        }

        // And the subscriber really receives what the topic publishes, in the form its
        // declaration asked for.
        let published_message = sns_ok(
            &router,
            row.case,
            "Publish",
            &[
                ("TopicArn", topic.clone()),
                ("Message", MESSAGE.to_string()),
            ],
        )
        .await;
        let message_id = xml_text(&published_message, "MessageId").unwrap_or_default();
        assert!(
            !message_id.is_empty(),
            "case {:?}: Publish must answer a MessageId, got {published_message}",
            row.case
        );
        for subscription in row.subscriptions {
            let delivered = drain(&router, row.case, &queue_url(subscription.queue_name)).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {:?}: {} must receive exactly the published message, got {delivered:?}",
                row.case,
                subscription.logical_id
            );
            let body = &delivered[0];
            if subscription.raw {
                assert_eq!(
                    body, MESSAGE,
                    "case {:?}: {} asked for raw delivery, so it must receive the body \
                     exactly, got {body:?}",
                    row.case, subscription.logical_id
                );
                continue;
            }
            let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!(
                    "case {:?}: {} must receive the notification envelope ({error}), got {body:?}",
                    row.case, subscription.logical_id
                )
            });
            assert_eq!(
                envelope["Message"].as_str(),
                Some(MESSAGE),
                "case {:?}: {}'s envelope must carry the published message, got {envelope}",
                row.case,
                subscription.logical_id
            );
            assert_eq!(
                envelope["TopicArn"].as_str(),
                Some(topic.as_str()),
                "case {:?}: {}'s envelope must name the stack's topic, got {envelope}",
                row.case,
                subscription.logical_id
            );
            assert_eq!(
                envelope["MessageId"].as_str(),
                Some(message_id.as_str()),
                "case {:?}: {}'s envelope must carry the id Publish answered, got {envelope}",
                row.case,
                subscription.logical_id
            );
        }
    }
}

/// A revision that stops declaring the subscription UNSUBSCRIBES it: the topic — which the
/// revision keeps — reports no subscriber, and the next publish reaches nothing.
#[tokio::test]
async fn a_revision_that_drops_the_subscription_unsubscribes_it() {
    let case = "a revision without the subscription";
    let router = build_router();
    let row = &ROWS[0];
    let template = template_of(row);
    let described = create_and_settle(&router, case, "dropped-subscriber-stack", &template).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: the stack must have created every resource it declares, got {described}"
    );
    let topic = topic_arn(row.topic_name);
    let queue = row.subscriptions[0].queue_name;

    // The same template with the subscription resource and its output taken out: the topic,
    // the queue and the grant stay.
    let mut revision = template.clone();
    let logical_id = row.subscriptions[0].logical_id;
    revision["Resources"]
        .as_object_mut()
        .expect("the template declares resources")
        .remove(logical_id);
    revision["Outputs"]
        .as_object_mut()
        .expect("the template declares outputs")
        .remove(logical_id);
    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", "dropped-subscriber-stack".to_string()),
            ("TemplateBody", revision.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let listed = sns_ok(
        &router,
        case,
        "ListSubscriptionsByTopic",
        &[("TopicArn", topic.clone())],
    )
    .await;
    assert!(
        xml_texts(&listed, "SubscriptionArn").is_empty(),
        "case {case:?}: the dropped subscription must be gone from the topic, got {listed}"
    );

    let published = sns_ok(
        &router,
        case,
        "Publish",
        &[
            ("TopicArn", topic.clone()),
            ("Message", MESSAGE.to_string()),
        ],
    )
    .await;
    assert!(
        xml_text(&published, "MessageId").is_some(),
        "case {case:?}: Publish must still be served for a topic with no subscriber, got \
         {published}"
    );
    let delivered = drain(&router, case, &queue_url(queue)).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: a publish after the unsubscribe must reach nothing, got {delivered:?}"
    );
}
