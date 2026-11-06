// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose template leaves a FIFO resource's name to CloudFormation
//! gets a name the owning service accepts.
//!
//! The rule under test: **a physical name CloudFormation GENERATES carries the suffix the
//! owning service requires of the variant the declaration asks for** — Amazon SNS and
//! Amazon SQS each require `.fifo` on the name of a FIFO resource ("For a FIFO
//! (first-in-first-out) topic, the name must end with the .fifo suffix",
//! `CreateTopicInput$Name`; "A FIFO queue name must end with the .fifo suffix",
//! `CreateQueueInput$QueueName`), so a generated name without it is one that service
//! refuses and the stack could never create the resource at all. A name the template
//! DECLARES is the caller's and goes through as written, refusal included (the suffix is
//! not appended to it).
//!
//! Two crates have to agree for that to hold (CloudFormation decides the name, SNS/SQS
//! accepts or refuses it), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: which type and which owning
//! service (`AWS::SNS::Topic`/`FifoTopic`, `AWS::SQS::Queue`/`FifoQueue`), how the
//! declaration spells the flag (a JSON boolean, the text a JSON template spells one as, a
//! `Ref` to a parameter that has to be evaluated, the flag off, the flag absent), whether
//! the template names the resource itself, and whether the generated name is short enough
//! to be used whole or long enough that the owning service's length bound has to fit it.
//! Every expected value is computed from what the row declares.
//!
//! The end-to-end half is the same rule read forwards: a stack whose FIFO topic, FIFO queue
//! and subscription are all unnamed completes, and the topic it created orders and
//! content-deduplicates the messages it delivers into the queue it created.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode};

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_500);

/// The suffix both owning services require on a FIFO resource's name.
const FIFO_SUFFIX: &str = ".fifo";

/// How many characters CloudFormation's generated-name unique segment carries.
const GENERATED_UNIQUE_LEN: usize = 12;

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

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
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
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon SNS operation (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// Call one Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
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
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// A `GetTopicAttributes` answer's `Attributes`, as the map a client reads it as: each
/// `<entry>`'s `<key>` paired with its own `<value>`.
fn topic_attributes_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "key")
        .into_iter()
        .zip(elements(xml, "value"))
        .collect()
}

/// A `GetQueueAttributes` answer's `Attributes`, as the map a client reads it as.
fn queue_attributes_of(answer: &Value) -> BTreeMap<String, String> {
    answer
        .get("Attributes")
        .and_then(Value::as_object)
        .map(|attributes| {
            attributes
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body beside the
/// reasons its events carry — which is what says WHY a resource failed.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> (String, Vec<String>) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    (body, elements(&events, "ResourceStatusReason"))
}

/// The ARN a topic named `name` carries in this emulator.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

/// Which service owns the row's resource, and so which call reads it back and which
/// property asks for its FIFO variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// `AWS::SNS::Topic`, whose FIFO property is `FifoTopic`.
    SnsTopic,
    /// `AWS::SQS::Queue`, whose FIFO property is `FifoQueue`.
    SqsQueue,
}

impl Owner {
    fn resource_type(self) -> &'static str {
        match self {
            Self::SnsTopic => "AWS::SNS::Topic",
            Self::SqsQueue => "AWS::SQS::Queue",
        }
    }

    /// The `Fn::GetAtt` attribute that reports the resource's NAME.
    fn name_attribute(self) -> &'static str {
        match self {
            Self::SnsTopic => "TopicName",
            Self::SqsQueue => "QueueName",
        }
    }

    /// The attribute the owning service reports the FIFO variant under.
    fn fifo_attribute(self) -> &'static str {
        match self {
            Self::SnsTopic => "FifoTopic",
            Self::SqsQueue => "FifoQueue",
        }
    }

    /// The bound the owning service publishes on a name of this type: topic names "must be
    /// between 1 and 256 characters long" (Amazon SNS API reference, `CreateTopic.Name`),
    /// and "A queue name can have up to 80 characters" (Amazon SQS API reference,
    /// `CreateQueue.QueueName`).
    fn name_limit(self) -> usize {
        match self {
            Self::SnsTopic => 256,
            Self::SqsQueue => 80,
        }
    }
}

/// One row: a template declaring ONE resource, and what the name it ends up with must be.
struct Row {
    case: &'static str,
    owner: Owner,
    stack_name: &'static str,
    logical_id: &'static str,
    /// The `Parameters` section the template carries, as JSON — `None` for a template with
    /// none.
    parameters: Option<&'static str>,
    /// The resource's `Properties`, as JSON.
    properties: &'static str,
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// Whether the declaration asks for the FIFO variant, so the name must end with the
    /// suffix its owning service requires.
    fifo: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "an unnamed FIFO topic whose flag is a JSON boolean",
        owner: Owner::SnsTopic,
        stack_name: "fifo-generated-topic-stack",
        logical_id: "Ordered",
        parameters: None,
        properties: r#"{"FifoTopic":true,"ContentBasedDeduplication":true}"#,
        declared_name: None,
        fifo: true,
    },
    Row {
        case: "an unnamed FIFO topic whose flag is the text a JSON template spells one as",
        owner: Owner::SnsTopic,
        stack_name: "fifo-generated-text-flag-stack",
        logical_id: "OrderedText",
        parameters: None,
        properties: r#"{"FifoTopic":"true"}"#,
        declared_name: None,
        fifo: true,
    },
    Row {
        case: "an unnamed topic whose FIFO flag is off, which is a standard topic",
        owner: Owner::SnsTopic,
        stack_name: "standard-flagged-topic-stack",
        logical_id: "Unordered",
        parameters: None,
        properties: r#"{"FifoTopic":false,"DisplayName":"standard"}"#,
        declared_name: None,
        fifo: false,
    },
    Row {
        case: "an unnamed topic that declares no FIFO flag at all",
        owner: Owner::SnsTopic,
        stack_name: "standard-topic-stack",
        logical_id: "Plain",
        parameters: None,
        properties: r#"{"DisplayName":"plain"}"#,
        declared_name: None,
        fifo: false,
    },
    Row {
        case: "a FIFO topic the template names itself, suffix included",
        owner: Owner::SnsTopic,
        stack_name: "named-fifo-topic-stack",
        logical_id: "Named",
        parameters: None,
        properties: r#"{"TopicName":"named-stack-ordered-topic.fifo","FifoTopic":true}"#,
        declared_name: Some("named-stack-ordered-topic.fifo"),
        fifo: true,
    },
    Row {
        case: "an unnamed FIFO queue, whose owning service is the other one",
        owner: Owner::SqsQueue,
        stack_name: "fifo-generated-queue-stack",
        logical_id: "OrderedQueue",
        parameters: None,
        properties: r#"{"FifoQueue":true,"ContentBasedDeduplication":true}"#,
        declared_name: None,
        fifo: true,
    },
    Row {
        case: "an unnamed FIFO queue whose flag is a Ref the deployment has to evaluate",
        owner: Owner::SqsQueue,
        stack_name: "fifo-parameterised-queue-stack",
        logical_id: "Parameterised",
        parameters: Some(r#"{"Ordered":{"Type":"String","Default":"true"}}"#),
        properties: r#"{"FifoQueue":{"Ref":"Ordered"},"ContentBasedDeduplication":true}"#,
        declared_name: None,
        fifo: true,
    },
    Row {
        // A stack name and a logical id whose generated name is far longer than the 80
        // characters a queue name may carry, so the fitting has to make room for the
        // suffix as well as for the unique segment.
        case: "an unnamed FIFO queue whose generated name has to be fitted to the bound",
        owner: Owner::SqsQueue,
        stack_name: "fifo-generated-queue-stack-with-a-deliberately-long-stack-name",
        logical_id: "OrderedQueueWithADeliberatelyLongLogicalId",
        parameters: None,
        properties: r#"{"FifoQueue":true}"#,
        declared_name: None,
        fifo: true,
    },
    Row {
        case: "a FIFO queue the template names itself, suffix included",
        owner: Owner::SqsQueue,
        stack_name: "named-fifo-queue-stack",
        logical_id: "NamedQueue",
        parameters: None,
        properties: r#"{"QueueName":"named-stack-ordered-queue.fifo","FifoQueue":true}"#,
        declared_name: Some("named-stack-ordered-queue.fifo"),
        fifo: true,
    },
    Row {
        case: "an unnamed standard queue, the other owning service's non-FIFO variant",
        owner: Owner::SqsQueue,
        stack_name: "standard-queue-stack",
        logical_id: "PlainQueue",
        parameters: None,
        properties: r#"{"VisibilityTimeout":45}"#,
        declared_name: None,
        fifo: false,
    },
];

/// The template one row declares: its resource, and the two outputs that publish the name
/// the resource carries and the handle its `Ref` resolves to.
fn template_of(row: &Row) -> String {
    let parameters = row
        .parameters
        .map(|section| format!(r#""Parameters":{section},"#))
        .unwrap_or_default();
    format!(
        r#"{{{parameters}"Resources":{{"{logical}":{{"Type":"{resource_type}",
             "Properties":{properties}}}}},
           "Outputs":{{"Name":{{"Value":{{"Fn::GetAtt":["{logical}","{name_attribute}"]}}}},
             "Handle":{{"Value":{{"Ref":"{logical}"}}}}}}}}"#,
        logical = row.logical_id,
        resource_type = row.owner.resource_type(),
        properties = row.properties,
        name_attribute = row.owner.name_attribute(),
    )
}

/// Whether `name` is the `<stack name>-<logical id>-<unique>` name CloudFormation generates
/// for a resource the template does not name, with the required suffix after it: the two
/// leading segments keep as much of themselves as the length bound left them, and the
/// unique segment is kept whole.
fn is_generated_name(name: &str, row: &Row) -> bool {
    let body = if row.fifo {
        match name.strip_suffix(FIFO_SUFFIX) {
            Some(body) => body,
            None => return false,
        }
    } else {
        name
    };
    let Some((head, unique)) = body.rsplit_once('-') else {
        return false;
    };
    let kept = |full: &str, segment: &str| {
        !segment.is_empty() && full.starts_with(segment) && !segment.ends_with('-')
    };
    unique.len() == GENERATED_UNIQUE_LEN
        && unique
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && head.rsplit_once('-').is_some_and(|(stack, logical)| {
            kept(row.stack_name, stack) && kept(row.logical_id, logical)
        })
}

#[tokio::test]
async fn a_generated_name_carries_the_suffix_the_declared_variant_requires() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (body, reasons) =
            create_and_settle(&router, case, row.stack_name, &template_of(row)).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body} with event reasons \
             {reasons:?}"
        );
        let outputs = outputs_of(&body);
        let name = outputs
            .get("Name")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Name output, got {outputs:?}"));

        // The name is the DECLARED one where the template supplied it, and the generated
        // form otherwise — which carries the suffix exactly when the row asked for the
        // FIFO variant.
        match row.declared_name {
            Some(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected the declared name, got {name:?}"
            ),
            None => assert!(
                is_generated_name(&name, row),
                "case {case:?}: expected the generated <stack>-<logical>-<unique> name, got \
                 {name:?}"
            ),
        }
        assert_eq!(
            name.ends_with(FIFO_SUFFIX),
            row.fifo,
            "case {case:?}: expected the {FIFO_SUFFIX} suffix present={} on the name, got \
             {name:?}",
            row.fifo
        );
        assert!(
            !name.is_empty() && name.chars().count() <= row.owner.name_limit(),
            "case {case:?}: expected a name within the {} characters {} publishes, got \
             {name:?} of {} characters",
            row.owner.name_limit(),
            row.owner.resource_type(),
            name.chars().count()
        );

        // And the owning service really holds it: the handle the stack published addresses
        // the resource that service created, which reports the variant the row asked for.
        let handle = outputs.get("Handle").cloned().unwrap_or_default();
        let reported = match row.owner {
            Owner::SnsTopic => {
                let arn = topic_arn(&name);
                assert_eq!(
                    handle, arn,
                    "case {case:?}: expected the Ref to resolve to the topic's own ARN, got \
                     {handle:?}"
                );
                let (status, answer) =
                    sns(&router, "GetTopicAttributes", &[("TopicArn", &arn)]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected SNS to hold the topic the stack published, got \
                     {status} with {answer}"
                );
                topic_attributes_of(&answer)
            }
            Owner::SqsQueue => {
                assert!(
                    handle.ends_with(&format!("/{name}")),
                    "case {case:?}: expected the Ref to resolve to the queue's own url, got \
                     {handle:?}"
                );
                let (status, answer) = sqs(
                    &router,
                    "GetQueueAttributes",
                    &json!({"QueueUrl": handle, "AttributeNames": ["All"]}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected SQS to hold the queue the stack published, got \
                     {status} with {answer}"
                );
                queue_attributes_of(&answer)
            }
        };
        let reports_fifo = reported
            .get(row.owner.fifo_attribute())
            .is_some_and(|value| value == "true");
        assert_eq!(
            reports_fifo, row.fifo,
            "case {case:?}: expected the owning service to report {} as FIFO={}, got \
             {reported:?}",
            row.owner.fifo_attribute(),
            row.fifo
        );
    }
}

/// The same rule read forwards, end to end: a stack whose FIFO topic, FIFO queue and
/// subscription are ALL unnamed completes, and the topic it created delivers into the queue
/// it created — in the order the publishes were made, with a repeated body deduplicated by
/// content ("ContentBasedDeduplication — Enables content-based deduplication for FIFO
/// topics", `SetTopicAttributes`). The stack's queue policy grants the SNS service principal
/// `sqs:SendMessage`, which is what lets a notification reach the queue at all ("you must
/// ensure that the Amazon SQS queue policy allows the Amazon SNS topic to send messages to
/// the queue").
#[tokio::test]
async fn an_unnamed_fifo_topic_and_queue_a_stack_created_deliver_in_order() {
    let router = build_router();
    let case = "a FIFO topic subscribed to a FIFO queue, every name generated";
    let stack_name = "fifo-pipeline-stack";
    let template = r#"{"Resources":{
           "Topic":{"Type":"AWS::SNS::Topic",
             "Properties":{"FifoTopic":true,"ContentBasedDeduplication":true}},
           "Queue":{"Type":"AWS::SQS::Queue",
             "Properties":{"FifoQueue":true,"ContentBasedDeduplication":true}},
           "QueuePolicy":{"Type":"AWS::SQS::QueuePolicy",
             "Properties":{"Queues":[{"Ref":"Queue"}],
               "PolicyDocument":{"Version":"2012-10-17",
                 "Statement":[{"Effect":"Allow",
                   "Principal":{"Service":"sns.amazonaws.com"},
                   "Action":"sqs:SendMessage",
                   "Resource":{"Fn::GetAtt":["Queue","Arn"]}}]}}},
           "Subscription":{"Type":"AWS::SNS::Subscription",
             "Properties":{"TopicArn":{"Ref":"Topic"},"Protocol":"sqs",
               "Endpoint":{"Fn::GetAtt":["Queue","Arn"]},"RawMessageDelivery":true}}},
         "Outputs":{"TopicArn":{"Value":{"Ref":"Topic"}},
           "QueueUrl":{"Value":{"Ref":"Queue"}},
           "SubscriptionArn":{"Value":{"Ref":"Subscription"}}}}"#;

    let (body, reasons) = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with event reasons \
         {reasons:?}"
    );
    let outputs = outputs_of(&body);
    let arn = outputs.get("TopicArn").cloned().unwrap_or_default();
    let url = outputs.get("QueueUrl").cloned().unwrap_or_default();
    let subscription = outputs.get("SubscriptionArn").cloned().unwrap_or_default();
    assert!(
        arn.ends_with(FIFO_SUFFIX) && url.ends_with(FIFO_SUFFIX),
        "case {case:?}: expected both generated names to carry the suffix their services \
         require, got {arn:?} and {url:?}"
    );

    // The subscription the stack published is one SNS answers for.
    let (status, answer) = sns(
        &router,
        "GetSubscriptionAttributes",
        &[("SubscriptionArn", &subscription)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SNS to hold the subscription the stack published, got \
         {status} with {answer}"
    );

    // Two distinct bodies in one message group, and the first body again: the repeat is the
    // same content, so the FIFO topic deduplicates it.
    let published = ["first", "second", "first"];
    for message in published {
        let (status, answer) = sns(
            &router,
            "Publish",
            &[
                ("TopicArn", &arn),
                ("Message", message),
                ("MessageGroupId", "the-one-group"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Publish of {message:?} to be accepted, got {status} \
             with {answer}"
        );
    }

    // What the queue holds, read until it stops answering with messages: a FIFO queue hands
    // them over in the order they arrived.
    let mut received: Vec<String> = Vec::new();
    for _ in 0..published.len() + 2 {
        let (status, answer) = sqs(
            &router,
            "ReceiveMessage",
            &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ReceiveMessage, got {status} with {answer}"
        );
        let empty = Vec::new();
        let batch = answer
            .get("Messages")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
            .clone();
        if batch.is_empty() {
            break;
        }
        for message in &batch {
            if let Some(body) = message.get("Body").and_then(Value::as_str) {
                received.push(body.to_string());
            }
            // Deleted, so the next receive answers with what follows rather than with the
            // same message once its visibility timeout runs out.
            if let Some(receipt) = message.get("ReceiptHandle").and_then(Value::as_str) {
                let (status, answer) = sqs(
                    &router,
                    "DeleteMessage",
                    &json!({"QueueUrl": url, "ReceiptHandle": receipt}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from DeleteMessage, got {status} with \
                     {answer}"
                );
            }
        }
    }
    assert_eq!(
        received,
        vec!["first".to_string(), "second".to_string()],
        "case {case:?}: expected the two distinct bodies in the order they were published, \
         the repeat deduplicated by content, got {received:?}"
    );
}
