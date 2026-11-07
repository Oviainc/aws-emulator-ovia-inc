// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares a FIFO Amazon SQS queue beside the IAM role and
//! the AWS Step Functions state machine integrated with it — driven across the four
//! services on ONE engine exactly as `core/router/src/lib.rs` mounts them.
//!
//! RULE: a stack every one of whose declared resources the emulator admits reaches
//! `CREATE_COMPLETE`, and each resource of a type the emulator provisions really exists
//! in the service that OWNS it — so a FIFO queue a stack declares is a queue Amazon SQS
//! answers for, with the attributes the template asked for, and the stack's outputs
//! address it.
//!
//! The rows differ along what the rule generalises over: whether the FIFO queue carries
//! content-based deduplication or the high-throughput attributes, which are the two
//! shapes of FIFO configuration a template declares. (A FIFO queue whose name
//! CloudFormation generates is `cloudformation_fifo_resource_generated_names.rs`, which
//! is the suite for that half.)
//!
//! Only the resources of PROVISIONED types are read back through their owning service.
//! The state machine is of a type this emulator records rather than creates, which is
//! what the open issues on its physical id and its lifetime are about; nothing here
//! asserts it exists in Step Functions, because that is not this suite's claim.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode, outputs_of};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// Long enough for a stack operation's clock-owed status to come due.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// How long a row waits for a stack to leave its in-progress status.
const PATIENCE: Duration = Duration::from_secs(20);

/// The four services this stack spans, over ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One CloudFormation call over its awsQuery wire: the status and the XML body.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, String)],
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

/// One Amazon SQS call over its awsJson1_0 wire, requiring a 200.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "expected 200 from SQS {operation}, got {status} with {parsed:?}"
    );
    parsed
}

/// The template one row deploys: a FIFO queue, the execution role of a state machine that
/// sends to it, and the state machine itself, with the queue's url and the machine's
/// handle published as outputs.
fn template(queue_name: &str, extra_queue_properties: Value) -> String {
    let mut queue_properties = json!({ "FifoQueue": true, "QueueName": queue_name });
    if let Some(extra) = extra_queue_properties.as_object() {
        for (property, value) in extra {
            queue_properties[property] = value.clone();
        }
    }
    json!({
        "Resources": {
            "Orders": { "Type": "AWS::SQS::Queue", "Properties": queue_properties },
            "WorkflowRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "states.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "Policies": [{
                        "PolicyName": "send",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sqs:SendMessage",
                                "Resource": { "Fn::GetAtt": ["Orders", "Arn"] }
                            }]
                        }
                    }]
                }
            },
            "Pipeline": {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "RoleArn": { "Fn::GetAtt": ["WorkflowRole", "Arn"] },
                    "DefinitionString": {
                        "Fn::Sub": [
                            "{\"StartAt\":\"Send\",\"States\":{\"Send\":{\"Type\":\"Task\",\
                             \"Resource\":\"arn:aws:states:::sqs:sendMessage\",\"Parameters\":\
                             {\"QueueUrl\":\"${Url}\",\"MessageBody\":\"ordered\",\
                             \"MessageGroupId\":\"orders\"},\"End\":true}}}",
                            { "Url": { "Ref": "Orders" } }
                        ]
                    }
                }
            }
        },
        "Outputs": {
            "QueueUrl": { "Value": { "Ref": "Orders" } },
            "QueueArn": { "Value": { "Fn::GetAtt": ["Orders", "Arn"] } },
            "RoleArn": { "Value": { "Fn::GetAtt": ["WorkflowRole", "Arn"] } },
            "Pipeline": { "Value": { "Ref": "Pipeline" } }
        }
    })
    .to_string()
}

/// One row: the case, the queue name the template declares, and the extra FIFO
/// properties it carries.
struct Row {
    case: &'static str,
    stack: &'static str,
    queue_name: &'static str,
    extra: Value,
    expected_attributes: Vec<(&'static str, &'static str)>,
}

/// Poll `DescribeStacks` until the stack leaves its in-progress status, and report the
/// status and the body it ended on.
async fn settled(router: &MultiServiceRouter, stack: &str, case: &str) -> (String, String) {
    let began = std::time::Instant::now();
    loop {
        let (status_code, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack.to_string())]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStacks to answer, got {status_code} with {body}"
        );
        let status = element(&body, "StackStatus");
        if !status.ends_with("_IN_PROGRESS") {
            return (status, body);
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a terminal stack status within {PATIENCE:?}, got {status}"
        );
        ::tokio::time::sleep(PAST_STACK_SPAN).await;
    }
}

/// A stack declaring a FIFO queue, a role and a state machine integrated with the queue
/// reaches CREATE_COMPLETE, and the queue it declared really exists in Amazon SQS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fifo_queue_and_state_machine_stack_creates_completely() {
    let rows = [
        Row {
            case: "a named FIFO queue with content-based deduplication",
            stack: "sfn-fifo-named",
            queue_name: "stack-orders.fifo",
            extra: json!({ "ContentBasedDeduplication": true, "VisibilityTimeout": 60 }),
            expected_attributes: vec![
                ("FifoQueue", "true"),
                ("ContentBasedDeduplication", "true"),
                ("VisibilityTimeout", "60"),
            ],
        },
        Row {
            case: "a named FIFO queue with the high-throughput attributes",
            stack: "sfn-fifo-throughput",
            queue_name: "stack-orders-fast.fifo",
            extra: json!({ "DeduplicationScope": "messageGroup", "FifoThroughputLimit": "perMessageGroupId" }),
            expected_attributes: vec![
                ("FifoQueue", "true"),
                ("DeduplicationScope", "messageGroup"),
                ("FifoThroughputLimit", "perMessageGroupId"),
            ],
        },
    ];

    let router = emulator();
    for row in rows {
        let case = row.case;
        let (status_code, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack.to_string()),
                ("TemplateBody", template(row.queue_name, row.extra.clone())),
                ("Capabilities.member.1", "CAPABILITY_IAM".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be admitted, got {status_code} with {body}"
        );

        let (status, described) = settled(&router, row.stack, case).await;
        // The stack's own events say which resource failed when one did, so a row that
        // rolls back says so rather than only naming the status.
        let (_events_status, events) = cloudformation(
            &router,
            "DescribeStackEvents",
            &[("StackName", row.stack.to_string())],
        )
        .await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: expected CREATE_COMPLETE, got {status}; events: {events}"
        );

        // The queue the stack declared is one Amazon SQS answers for, with the attributes
        // the template asked for: the stack's output addresses the real queue.
        let outputs = outputs_of(&described);
        let url = outputs
            .get("QueueUrl")
            .unwrap_or_else(|| panic!("case {case:?}: expected the queue url in {outputs:?}"));
        let attributes = sqs(
            &router,
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["All"] }),
        )
        .await;
        for (attribute, expected) in &row.expected_attributes {
            assert_eq!(
                attributes["Attributes"][attribute].as_str(),
                Some(*expected),
                "case {case:?}: expected the queue's {attribute} to be {expected:?}, got \
                 {attributes:?}"
            );
        }
        // A FIFO queue's name ends in the suffix Amazon SQS requires of one, which the
        // ARN the stack publishes for it carries.
        let queue_arn = outputs
            .get("QueueArn")
            .unwrap_or_else(|| panic!("case {case:?}: expected the queue ARN in {outputs:?}"));
        assert!(
            queue_arn.ends_with(".fifo"),
            "case {case:?}: expected a FIFO queue ARN, got {queue_arn}"
        );
        // The message plane of the real queue: a send with a group id and a deduplication
        // id is accepted, which is what the state machine the stack declared would make.
        sqs(
            &router,
            "SendMessage",
            &json!({
                "QueueUrl": url,
                "MessageBody": "ordered",
                "MessageGroupId": "orders",
                "MessageDeduplicationId": "ordered-once",
            }),
        )
        .await;
    }
}
