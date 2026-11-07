// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an SNS topic, a Lambda function and an
//! `AWS::SNS::Subscription` with `Protocol: lambda` really wires them together: a `Publish` to
//! the stack's topic INVOKES the stack's function, and the function's own handler proves it by
//! writing into a queue this suite reads.
//!
//! The rule under test: **SNS delivers each published message to every endpoint subscribed to
//! the topic, in the form that endpoint's protocol defines — and a delivery to another
//! service's resource is made only where that resource's own policy grants it.** For the
//! `lambda` protocol that is the function's resource-based policy: the statement an
//! `AWS::Lambda::Permission` declares (`lambda:InvokeFunction` for `sns.amazonaws.com`, scoped
//! to the topic). Four crates have to agree for that to hold — CloudFormation makes the calls,
//! SNS performs the delivery, Lambda runs the handler and SQS receives what it wrote — so the
//! suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the function's policy carries
//! the grant, and whether the template NAMES its resources at all or leaves every physical name
//! to CloudFormation, as a synthesized app's template does — the delivery addresses the function
//! by the handle the stack published either way. The second suite is the same rule across a
//! REVISION of the deployed stack: one that adds the grant starts the delivery and one that
//! drops it stops it again.
//!
//! `sns_lambda_subscription_permission_boundary.rs` is this rule's sibling over the two-leg
//! deployment a client validates a wiring by; what this suite adds is the generated-name shape
//! and the revisions.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire readers are the suite-shared ones, not a second copy: the form encoding a
// template body survives, the element reader, and the `Outputs` map of a `DescribeStacks`
// answer.
use crate::query_wire::{form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-west-2";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How long this suite watches the stack for `CREATE_COMPLETE` and the sink queue for what the
/// invoked handler wrote: both are owed by the clock (the stack's provisioning span, the
/// function's activation, the asynchronous invocation), so nothing here sleeps on the
/// emulator's behalf beyond looking again.
const WATCH_BUDGET: Duration = Duration::from_secs(40);

/// How long to look again after a stack that is still provisioning or a sink still empty.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// How long the leg that must NOT be invoked is given after the leg that must: both legs'
/// deliveries are decided in the same way, so a sink still empty after the granted one has
/// arrived was never written to.
const SETTLE: Duration = Duration::from_secs(2);

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
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

/// One CloudFormation operation that must be answered.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, String)],
) -> String {
    let (status, body) = query(router, "cloudformation", "2010-05-15", action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// One Amazon SNS operation that must be answered.
async fn sns(
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

/// One Amazon SQS operation over its `awsJson1_0` wire that must be answered.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) -> Value {
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

/// The text of the first `<tag>` element of an XML body, read with the suite-shared reader.
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    opt_element(xml, tag)
}

/// The handler the stack's function runs: it forwards the WHOLE event it was handed into the
/// queue the stack declared beside it, so an invocation is observed through the effect the
/// handler itself had.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3


def handler(event, context):
    boto3.client("sqs").send_message(
        QueueUrl=os.environ["SINK_QUEUE_URL"],
        MessageBody=json.dumps(event),
    )
    return {"records": len(event["Records"])}
"#;

/// The role a Lambda function assumes, as a template declares it: the trust policy that admits
/// the Lambda service.
fn lambda_trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The template one leg deploys: the sink queue, the execution role, the topic, the function
/// whose handler forwards into the sink, the `lambda`-protocol subscription binding them — and
/// the `AWS::Lambda::Permission` only when `with_permission` asks for it.
///
/// A row whose `names` declares none leaves every physical name to CloudFormation, which is
/// what a synthesized app's template does: the stack's own handles (`Ref`, `Fn::GetAtt`) are
/// then the only way its resources are addressed.
fn template_of(row: &Row) -> Value {
    let mut resources = json!({
        "Sink": {
            "Type": "AWS::SQS::Queue",
            "Properties": named("QueueName", row.declared_names.map(|names| names.sink)),
        },
        "Role": {
            "Type": "AWS::IAM::Role",
            "Properties": with_name(
                json!({ "AssumeRolePolicyDocument": lambda_trust_policy() }),
                "RoleName",
                row.declared_names.map(|names| names.role),
            ),
        },
        "Topic": {
            "Type": "AWS::SNS::Topic",
            "Properties": named("TopicName", row.declared_names.map(|names| names.topic)),
        },
        "Consumer": {
            "Type": "AWS::Lambda::Function",
            "Properties": with_name(
                json!({
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 10,
                    "Role": {"Fn::GetAtt": ["Role", "Arn"]},
                    "Environment": {"Variables": {"SINK_QUEUE_URL": {"Ref": "Sink"}}},
                    "Code": {"ZipFile": HANDLER_SOURCE},
                }),
                "FunctionName",
                row.declared_names.map(|names| names.function),
            ),
        },
        "Subscription": {
            "Type": "AWS::SNS::Subscription",
            "Properties": {
                "TopicArn": {"Ref": "Topic"},
                "Protocol": "lambda",
                "Endpoint": {"Fn::GetAtt": ["Consumer", "Arn"]},
            },
        },
    });
    if row.with_permission {
        resources
            .as_object_mut()
            .expect("the resources are an object")
            .insert(
                "InvokePermission".to_string(),
                json!({
                    "Type": "AWS::Lambda::Permission",
                    "Properties": {
                        "Action": "lambda:InvokeFunction",
                        "FunctionName": {"Fn::GetAtt": ["Consumer", "Arn"]},
                        "Principal": "sns.amazonaws.com",
                        "SourceArn": {"Ref": "Topic"},
                    },
                }),
            );
    }
    json!({
        "Resources": resources,
        "Outputs": {
            "TopicArn": {"Value": {"Ref": "Topic"}},
            "FunctionName": {"Value": {"Ref": "Consumer"}},
            "SinkQueueUrl": {"Value": {"Ref": "Sink"}},
        },
    })
}

/// `Properties` holding just the name member `member`, when the row declares one, and no
/// property at all when it leaves the name to CloudFormation.
fn named(member: &str, name: Option<&str>) -> Value {
    with_name(json!({}), member, name)
}

/// `properties` plus the name member `member`, when the row declares a name.
fn with_name(mut properties: Value, member: &str, name: Option<&str>) -> Value {
    if let (Some(target), Some(name)) = (properties.as_object_mut(), name) {
        target.insert(member.to_string(), Value::String(name.to_string()));
    }
    properties
}

/// The names one row's resources are declared under, for a row that declares them at all.
struct Names {
    sink: &'static str,
    role: &'static str,
    topic: &'static str,
    function: &'static str,
}

/// One row: a deployment of the same app, and what the publish to its topic must come to.
#[derive(Clone, Copy)]
struct Row {
    case: &'static str,
    /// The stack's own name, which a caller always chooses.
    stack: &'static str,
    /// The physical names the template declares, or `None` to leave every one of them to
    /// CloudFormation.
    declared_names: Option<&'static Names>,
    /// Whether the stack declares the `AWS::Lambda::Permission` that grants this service the
    /// invocation.
    with_permission: bool,
    /// How many times the published message must reach the function's handler.
    invocations: usize,
}

/// Create the stack and watch it until it leaves `CREATE_IN_PROGRESS`, answering its last
/// `DescribeStacks` body.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &Value,
) -> String {
    cloudformation(
        router,
        case,
        "CreateStack",
        &[
            ("StackName", stack.to_string()),
            ("TemplateBody", template.to_string()),
            // A template that NAMES the execution role it declares is the capability
            // CloudFormation requires a caller to acknowledge; it is acknowledged for every
            // row, because a row that leaves the name to CloudFormation declares the same
            // kind of resource.
            (
                "Capabilities.member.1",
                "CAPABILITY_NAMED_IAM".to_string(),
            ),
        ],
    )
    .await;
    let deadline = Instant::now() + WATCH_BUDGET;
    loop {
        let described = cloudformation(
            router,
            case,
            "DescribeStacks",
            &[("StackName", stack.to_string())],
        )
        .await;
        let status = xml_text(&described, "StackStatus").unwrap_or_default();
        if status != "CREATE_IN_PROGRESS" || Instant::now() >= deadline {
            return described;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every message the queue at `url` holds right now, as the JSON its body carries.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    loop {
        let received = sqs(
            router,
            case,
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

/// Watch the queue at `url` until it holds at least one event, answering what it held.
async fn awaited_events(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        seen.extend(drain(router, case, url).await);
        if !seen.is_empty() || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The prefix of an SNS topic ARN of this account and Region, which every row's published
/// `TopicArn` has to carry whether the template named the topic or left the name to the stack.
fn topic_arn_prefix() -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:")
}

/// The names the rows that declare them use.
const GRANTED_NAMES: Names = Names {
    sink: "sns-lambda-granted-sink",
    role: "sns-lambda-granted-role",
    topic: "sns-lambda-granted-topic",
    function: "sns-lambda-granted-consumer",
};
const WITHHELD_NAMES: Names = Names {
    sink: "sns-lambda-withheld-sink",
    role: "sns-lambda-withheld-role",
    topic: "sns-lambda-withheld-topic",
    function: "sns-lambda-withheld-consumer",
};

/// RULE: a stack-declared `lambda`-protocol subscription has its function invoked by a publish
/// to the stack's topic exactly where the function's own policy grants this service the
/// invocation — the statement the stack's `AWS::Lambda::Permission` declares — and the publish
/// answers its `MessageId` either way.
///
/// The rows differ along what the rule generalises over: whether the function's policy carries
/// the grant, and whether the template names its resources at all or leaves every physical name
/// to CloudFormation, as a synthesized app's template does — the delivery addresses the function
/// by the handle the stack published either way.
#[tokio::test]
async fn a_stack_declared_lambda_subscription_is_invoked_only_where_its_policy_grants_it() {
    let rows: &[Row] = &[
        Row {
            case: "the stack that declares the invoke permission",
            stack: "sns-lambda-granted",
            declared_names: Some(&GRANTED_NAMES),
            with_permission: true,
            invocations: 1,
        },
        Row {
            case: "the same stack with every physical name left to CloudFormation",
            stack: "sns-lambda-generated",
            declared_names: None,
            with_permission: true,
            invocations: 1,
        },
        Row {
            case: "the identical stack that declares no invoke permission",
            stack: "sns-lambda-withheld",
            declared_names: Some(&WITHHELD_NAMES),
            with_permission: false,
            invocations: 0,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = build_router();
        let template = template_of(row);
        let described = deployed(&router, case, row.stack, &template).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: the stack must create every resource it declares, got {described}"
        );
        let outputs = outputs_of(&described);
        let topic = outputs.get("TopicArn").cloned().unwrap_or_default();
        assert!(
            topic
                .strip_prefix(&topic_arn_prefix())
                .is_some_and(|name| !name.is_empty()),
            "case {case:?}: the stack must publish its topic's own ARN, got {outputs:?}"
        );
        let sink = outputs.get("SinkQueueUrl").cloned().unwrap_or_default();
        assert!(
            !sink.is_empty(),
            "case {case:?}: the stack must publish its sink queue's url, got {outputs:?}"
        );

        let marker = format!("marker-{}", row.stack);
        let published = sns(
            &router,
            case,
            "Publish",
            &[("TopicArn", topic.clone()), ("Message", marker.clone())],
        )
        .await;
        let message_id = xml_text(&published, "MessageId").unwrap_or_default();
        assert!(
            !message_id.is_empty(),
            "case {case:?}: Publish must answer a MessageId whether or not the subscribed \
             function could be invoked, got {published}"
        );

        let mut events = if row.invocations > 0 {
            awaited_events(&router, case, &sink).await
        } else {
            Vec::new()
        };
        tokio::time::sleep(SETTLE).await;
        events.extend(drain(&router, case, &sink).await);
        assert_eq!(
            events.len(),
            row.invocations,
            "case {case:?}: expected {} invocation(s) of the stack's function, got {events:?}",
            row.invocations
        );
        let Some(event) = events.first() else {
            continue;
        };
        let record = event["Records"][0].clone();
        assert_eq!(
            record["EventSource"].as_str(),
            Some("aws:sns"),
            "case {case:?}: the handler must be handed the notification as an SNS record, got \
             {event}"
        );
        assert_eq!(
            record["Sns"]["Message"].as_str(),
            Some(marker.as_str()),
            "case {case:?}: the record must carry the message as published, got {event}"
        );
        assert_eq!(
            record["Sns"]["TopicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the record must name the stack's topic, got {event}"
        );
        assert_eq!(
            record["Sns"]["MessageId"].as_str(),
            Some(message_id.as_str()),
            "case {case:?}: the record must carry the id the publish answered, got {event}"
        );
    }
}

/// RULE, on the other side of the same gate: a REVISION of the deployed stack that adds the
/// `AWS::Lambda::Permission` statement makes the next publish reach the function, and one that
/// drops it stops the delivery again — the grant the function's policy carries at publish time
/// is what decides each delivery, not the one it carried when the subscription was made.
#[tokio::test]
async fn a_revision_that_adds_the_invoke_permission_starts_the_delivery() {
    let case = "the deployed stack revised to grant, then to revoke, the invocation";
    let router = build_router();
    let row = Row {
        case,
        stack: "sns-lambda-revised",
        declared_names: Some(&REVISED_NAMES),
        with_permission: false,
        invocations: 0,
    };

    // As deployed: the subscription is there, the grant is not.
    let described = deployed(&router, case, row.stack, &template_of(&row)).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: the stack must create every resource it declares, got {described}"
    );
    let outputs = outputs_of(&described);
    let topic = outputs.get("TopicArn").cloned().unwrap_or_default();
    let sink = outputs.get("SinkQueueUrl").cloned().unwrap_or_default();

    // (the leg, the revision's template, how many invocations that leg's publish must cause)
    let legs: &[(&str, bool, usize)] = &[
        ("as deployed, with no grant", false, 0),
        ("revised to add the grant", true, 1),
        ("revised again to drop the grant", false, 0),
    ];

    for (index, (leg, with_permission, invocations)) in legs.iter().enumerate() {
        if index > 0 {
            let revision = template_of(&Row {
                with_permission: *with_permission,
                ..row
            });
            let updated = cloudformation(
                &router,
                leg,
                "UpdateStack",
                &[
                    ("StackName", row.stack.to_string()),
                    ("TemplateBody", revision.to_string()),
                    (
                        "Capabilities.member.1",
                        "CAPABILITY_NAMED_IAM".to_string(),
                    ),
                ],
            )
            .await;
            assert!(
                xml_text(&updated, "StackId").is_some(),
                "case {leg:?}: UpdateStack must answer the stack it revised, got {updated}"
            );
            // The revision's own provisioning span is owed by the clock.
            let deadline = Instant::now() + WATCH_BUDGET;
            loop {
                let described = cloudformation(
                    &router,
                    leg,
                    "DescribeStacks",
                    &[("StackName", row.stack.to_string())],
                )
                .await;
                let status = xml_text(&described, "StackStatus").unwrap_or_default();
                if status == "UPDATE_COMPLETE" || Instant::now() >= deadline {
                    assert_eq!(
                        status, "UPDATE_COMPLETE",
                        "case {leg:?}: the revision must be applied, got {described}"
                    );
                    break;
                }
                tokio::time::sleep(LOOK_AGAIN).await;
            }
        }

        let marker = format!("marker-{}", leg.replace(' ', "-"));
        sns(
            &router,
            leg,
            "Publish",
            &[("TopicArn", topic.clone()), ("Message", marker.clone())],
        )
        .await;
        let mut events = if *invocations > 0 {
            awaited_events(&router, leg, &sink).await
        } else {
            Vec::new()
        };
        tokio::time::sleep(SETTLE).await;
        events.extend(drain(&router, leg, &sink).await);
        assert_eq!(
            events.len(),
            *invocations,
            "case {leg:?}: expected {invocations} invocation(s) of the stack's function, got \
             {events:?}"
        );
        if let Some(event) = events.first() {
            assert_eq!(
                event["Records"][0]["Sns"]["Message"].as_str(),
                Some(marker.as_str()),
                "case {leg:?}: the record must carry the message this leg published, got {event}"
            );
        }
    }
}

/// The names the revised stack's resources carry.
const REVISED_NAMES: Names = Names {
    sink: "sns-lambda-revised-sink",
    role: "sns-lambda-revised-role",
    topic: "sns-lambda-revised-topic",
    function: "sns-lambda-revised-consumer",
};
