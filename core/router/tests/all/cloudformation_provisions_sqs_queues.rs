// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::SQS::Queue` resources its template declares — in SQS.
//!
//! The rule under test: **a stack reaches its completed status only once the service that
//! owns each declared resource has created it, and the handle the stack publishes for that
//! resource (`Ref`/`Fn::GetAtt`) resolves in that service.** For a queue the handle is its
//! url, so after the create waiter returns, `SendMessage`/`ReceiveMessage`/`DeleteMessage`
//! and `GetQueueAttributes` on the stack's own output succeed — rather than SQS answering
//! `AWS.SimpleQueueService.NonExistentQueue` for a url its own stack just published. Two
//! crates have to agree for that to hold (CloudFormation makes the call, SQS serves it),
//! so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! queue or leaves the name to CloudFormation, how many queues one stack declares, which
//! attributes the properties set (and whether they are written as numbers, booleans,
//! strings or an object), whether the queue is standard or FIFO, and whether one queue's
//! property refers to another queue of the same stack. Every expected value is computed
//! from what the row declares, never read back out of the answer being checked.
//!
//! The deletion half and the refusal half are rows of the same rule: deleting the stack
//! deletes the queues it created, and a queue SQS REFUSES to create leaves the stack
//! `CREATE_FAILED` naming the resource rather than publishing a dangling url.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the stack's queues and
/// their ARNs belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due. A queue has no transient status of its own: SQS answers `CreateQueue` with
/// a queue that is usable at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
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

/// Percent-encode one `application/x-www-form-urlencoded` value the way an AWS SDK does,
/// so a template body full of `{`, `"` and `&` survives the body.
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

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
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

/// One SQS call that must succeed.
async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = sqs(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of an SQS refusal: awsJson1_0 carries it as the body's `__type`.
fn sqs_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
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
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
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
    body
}

/// The url a queue named `name` is addressed at in this emulator.
fn queue_url(name: &str) -> String {
    format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/{name}")
}

/// The ARN a queue named `name` carries.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// What the row says about one queue the template declares.
struct DeclaredQueue {
    /// The output key whose value is the queue's url (`{"Ref": <logical id>}`).
    url_output: &'static str,
    /// The resource's logical id, which the generated name is built from.
    logical_id: &'static str,
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// Attributes `GetQueueAttributes` must report for the created queue, computed from
    /// what the row's template declares.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// Whether this row exercises the message plane on the queue's url.
    ///
    /// Every created queue is read back through `GetQueueUrl`/`GetQueueAttributes`; a
    /// send/receive/delete round trip is asserted wherever SQS serves one. It is not for
    /// a FIFO queue, because SQS's own `SendMessage` to a FIFO queue is still one of this
    /// emulator's 501 stubs — which is that operation's own issue, not this stack's.
    message_plane: bool,
}

/// One row: a template and the queues it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    queues: &'static [DeclaredQueue],
}

const ROWS: &[Row] = &[
    Row {
        case: "an unnamed queue with no properties, whose name CloudFormation generates",
        stack_name: "unnamed-queue-stack",
        template: r#"{"Resources":{"Queue":{"Type":"AWS::SQS::Queue"}},
             "Outputs":{"QueueUrl":{"Value":{"Ref":"Queue"}}}}"#,
        queues: &[DeclaredQueue {
            url_output: "QueueUrl",
            logical_id: "Queue",
            declared_name: None,
            // The documented CreateQueue defaults, which a template that sets nothing
            // leaves the queue with.
            expected_attributes: &[
                ("VisibilityTimeout", "30"),
                ("MessageRetentionPeriod", "345600"),
                ("DelaySeconds", "0"),
                ("MaximumMessageSize", "1048576"),
                ("ReceiveMessageWaitTimeSeconds", "0"),
            ],
            message_plane: true,
        }],
    },
    Row {
        case: "a named queue whose properties are attributes written as numbers and tags",
        stack_name: "named-queue-stack",
        template: r#"{"Resources":{"Events":{"Type":"AWS::SQS::Queue",
               "Properties":{"QueueName":"stack-events-queue","VisibilityTimeout":120,
                 "MessageRetentionPeriod":600,"DelaySeconds":5,
                 "ReceiveMessageWaitTimeSeconds":10,
                 "Tags":[{"Key":"team","Value":"platform"}]}}},
             "Outputs":{"EventsUrl":{"Value":{"Ref":"Events"}},
               "EventsArn":{"Value":{"Fn::GetAtt":["Events","Arn"]}},
               "EventsName":{"Value":{"Fn::GetAtt":["Events","QueueName"]}}}}"#,
        queues: &[DeclaredQueue {
            url_output: "EventsUrl",
            logical_id: "Events",
            declared_name: Some("stack-events-queue"),
            expected_attributes: &[
                ("VisibilityTimeout", "120"),
                ("MessageRetentionPeriod", "600"),
                ("DelaySeconds", "5"),
                ("ReceiveMessageWaitTimeSeconds", "10"),
            ],
            message_plane: true,
        }],
    },
    Row {
        case: "a FIFO queue, whose properties are booleans and a value set",
        stack_name: "fifo-queue-stack",
        template: r#"{"Resources":{"Ordered":{"Type":"AWS::SQS::Queue",
               "Properties":{"QueueName":"stack-ordered-queue.fifo","FifoQueue":true,
                 "ContentBasedDeduplication":true,"DeduplicationScope":"queue",
                 "FifoThroughputLimit":"perQueue"}}},
             "Outputs":{"OrderedUrl":{"Value":{"Ref":"Ordered"}}}}"#,
        queues: &[DeclaredQueue {
            url_output: "OrderedUrl",
            logical_id: "Ordered",
            declared_name: Some("stack-ordered-queue.fifo"),
            expected_attributes: &[
                ("FifoQueue", "true"),
                ("ContentBasedDeduplication", "true"),
                ("DeduplicationScope", "queue"),
                ("FifoThroughputLimit", "perQueue"),
            ],
            message_plane: false,
        }],
    },
    Row {
        case: "two queues, one a dead-letter target the other's RedrivePolicy object names",
        stack_name: "redrive-queue-stack",
        template: r#"{"Resources":{
               "DeadLetters":{"Type":"AWS::SQS::Queue",
                 "Properties":{"QueueName":"stack-dead-letters"}},
               "Work":{"Type":"AWS::SQS::Queue",
                 "Properties":{"QueueName":"stack-work","VisibilityTimeout":60,
                   "RedrivePolicy":{"maxReceiveCount":3,
                     "deadLetterTargetArn":{"Fn::GetAtt":["DeadLetters","Arn"]}}}}},
             "Outputs":{"WorkUrl":{"Value":{"Ref":"Work"}},
               "DeadLettersUrl":{"Value":{"Ref":"DeadLetters"}}}}"#,
        queues: &[
            DeclaredQueue {
                url_output: "WorkUrl",
                logical_id: "Work",
                declared_name: Some("stack-work"),
                expected_attributes: &[
                    ("VisibilityTimeout", "60"),
                    // The object the template declared, as the compact JSON string the
                    // attribute is documented to carry — the members in the order the
                    // template wrote them, with the intrinsic resolved to the other
                    // queue's ARN.
                    (
                        "RedrivePolicy",
                        r#"{"maxReceiveCount":3,"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:stack-dead-letters"}"#,
                    ),
                ],
                message_plane: true,
            },
            DeclaredQueue {
                url_output: "DeadLettersUrl",
                logical_id: "DeadLetters",
                declared_name: Some("stack-dead-letters"),
                expected_attributes: &[("VisibilityTimeout", "30")],
                message_plane: true,
            },
        ],
    },
];

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for
/// a resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

#[tokio::test]
async fn a_completed_stack_has_created_every_queue_it_declares_in_sqs() {
    let router = build_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, row.template).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for queue in row.queues {
            let case = format!("{} [{}]", row.case, queue.logical_id);
            let url = outputs
                .get(queue.url_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", queue.url_output)
                })
                .clone();

            // The name the stack gave the queue: the declared one, or CloudFormation's
            // generated `<stack>-<logical id>-<suffix>`.
            let name = match queue.declared_name {
                Some(declared) => {
                    assert_eq!(
                        url,
                        queue_url(declared),
                        "case {case:?}: expected the Ref to be the declared queue's url"
                    );
                    declared.to_string()
                }
                None => {
                    let name = url.rsplit('/').next().unwrap_or_default().to_string();
                    assert_eq!(
                        url,
                        queue_url(&name),
                        "case {case:?}: expected a queue url of this account, got {url}"
                    );
                    assert!(
                        is_generated_name(&name, row.stack_name, queue.logical_id),
                        "case {case:?}: expected a generated <stack>-<logical id>-<suffix> \
                         name, got {name:?}"
                    );
                    name
                }
            };

            // SQS itself resolves the name the stack gave the queue, and answers the same
            // url: the two halves agree about which queue exists.
            let resolved = sqs_ok(
                &router,
                &case,
                "GetQueueUrl",
                json!({"QueueName": name}),
            )
            .await;
            assert_eq!(
                resolved.get("QueueUrl").and_then(Value::as_str),
                Some(url.as_str()),
                "case {case:?}: expected SQS to resolve the stack's queue name to its url, \
                 got {resolved}"
            );

            // The configuration the template declared is the configuration the queue has.
            let reported = sqs_ok(
                &router,
                &case,
                "GetQueueAttributes",
                json!({"QueueUrl": url, "AttributeNames": ["All"]}),
            )
            .await;
            let attributes = reported
                .get("Attributes")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                attributes.get("QueueArn").and_then(Value::as_str),
                Some(queue_arn(&name).as_str()),
                "case {case:?}: expected the created queue's own ARN, got {attributes}"
            );
            for (attribute, value) in queue.expected_attributes {
                assert_eq!(
                    attributes.get(*attribute).and_then(Value::as_str),
                    Some(*value),
                    "case {case:?}: expected {attribute}={value}, got {attributes}"
                );
            }

            // And the message plane works on the url the stack published: a send, a
            // receive of the same body, and a delete of what was received.
            if !queue.message_plane {
                continue;
            }
            let payload = format!("hello {}", queue.logical_id);
            let sent = sqs_ok(
                &router,
                &case,
                "SendMessage",
                json!({"QueueUrl": url, "MessageBody": payload}),
            )
            .await;
            assert!(
                sent.get("MessageId").and_then(Value::as_str).is_some(),
                "case {case:?}: expected a MessageId from SendMessage, got {sent}"
            );
            let received = sqs_ok(
                &router,
                &case,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
            )
            .await;
            let messages = received
                .get("Messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                messages.len(),
                1,
                "case {case:?}: expected the one message that was sent, got {received}"
            );
            assert_eq!(
                messages[0].get("Body").and_then(Value::as_str),
                Some(payload.as_str()),
                "case {case:?}: expected the body that was sent, got {received}"
            );
            let handle = messages[0]
                .get("ReceiptHandle")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no ReceiptHandle in {received}"));
            sqs_ok(
                &router,
                &case,
                "DeleteMessage",
                json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_queues_it_created() {
    let router = build_router();
    let case = "two queues deleted with their stack";
    let stack_name = "deleted-queue-stack";
    let template = r#"{"Resources":{
           "Kept":{"Type":"AWS::SQS::Queue","Properties":{"QueueName":"deleted-stack-kept"}},
           "Other":{"Type":"AWS::SQS::Queue"}},
         "Outputs":{"KeptUrl":{"Value":{"Ref":"Kept"}},
           "OtherUrl":{"Value":{"Ref":"Other"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let urls: Vec<String> = ["KeptUrl", "OtherUrl"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for url in &urls {
        sqs_ok(
            &router,
            case,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
        )
        .await;
    }

    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // Every queue the stack created is gone from SQS, which is what makes the stack's
    // resources the stack's own rather than leftovers.
    for url in &urls {
        let (status, answer) = sqs(&router, "ReceiveMessage", &json!({"QueueUrl": url})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the queue at {url} to be gone, got {status} with {answer}"
        );
        assert_eq!(
            sqs_error_code(&answer),
            "QueueDoesNotExist",
            "case {case:?}: expected QueueDoesNotExist for {url}, got {answer}"
        );
    }
}

#[tokio::test]
async fn a_queue_sqs_refuses_to_create_fails_the_stack() {
    let router = build_router();
    let case = "a visibility timeout outside the documented range";
    let stack_name = "refused-queue-stack";
    // "Valid values: An integer from 0 to 43,200 (12 hours)" — a value SQS's own
    // CreateQueue refuses, which is what the stack must report rather than completing.
    let template = r#"{"Resources":{"Queue":{"Type":"AWS::SQS::Queue",
           "Properties":{"QueueName":"refused-stack-queue","VisibilityTimeout":99999}}},
         "Outputs":{"QueueUrl":{"Value":{"Ref":"Queue"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail rather than publish a queue url \
         nothing answers to, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Queue") && reason.contains("InvalidAttributeValue"),
        "case {case:?}: expected the reason to name the resource and SQS's own refusal, \
         got {reason:?}"
    );

    // And nothing was created: the name the template asked for resolves to no queue.
    let (status, answer) = sqs(
        &router,
        "GetQueueUrl",
        &json!({"QueueName": "refused-stack-queue"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected no queue of that name, got {status} with {answer}"
    );
    assert_eq!(
        sqs_error_code(&answer),
        "QueueDoesNotExist",
        "case {case:?}: expected QueueDoesNotExist, got {answer}"
    );
}
