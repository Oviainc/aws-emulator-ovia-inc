// SPDX-License-Identifier: CC0-1.0

//! A CDK-shaped stack whose SNS topic carries a `lambda`-protocol subscription invokes the
//! subscribed function on every publish, and only where the function's own resource policy
//! grants Amazon SNS that invocation for the publishing topic.
//!
//! The rule under test: **a publish to a topic delivers the message to every confirmed
//! `lambda`-protocol subscription of that topic ("lambda – delivery of JSON-encoded message to
//! an Lambda function", `SubscribeInput$Protocol`) by invoking the subscribed function
//! asynchronously with the message in the `Records` envelope a function's event carries — and
//! that invocation is made only where the FUNCTION's own resource-based policy admits
//! `lambda:InvokeFunction` for the service principal `sns.amazonaws.com` and the publishing
//! topic, whatever a stack's own `AWS::Lambda::Permission` happens to declare.**
//!
//! It is the same rule `sns_lambda_subscription_permission_boundary.rs` pins, asserted over
//! the shape a SYNTHESIZED stack carries rather than a hand-written one: the function is left
//! UNNAMED (so its name and ARN are the ones CloudFormation mints), its execution role and
//! that role's inline policy are declared by the same stack, the sink the handler forwards to
//! is a queue created OUTSIDE the stack and handed to the function in its environment, and the
//! template carries the bootstrap-version parameter, the rule over it, the metadata resource
//! and its condition that `cdk synth` emits. Each row is a stack of its own with a sink of its
//! own, which is how a client deploys the two halves.
//!
//! Every observation is made the instant the stack reports itself complete, with nothing
//! waited for in between, which is where the second rule holds: **a resource a stack reports
//! created is one the service that owns it takes calls on** — so the function the stack
//! published is invocable, rather than one AWS Lambda still refuses every call on because its
//! own `State` is `Pending` ("During this time, you can't invoke or modify the function"). A
//! publish that reaches a function that cannot be invoked yet delivers nothing, so the two
//! rules are observed together here.
//!
//! The observation is made where a client makes it: on the sink queue the function forwards
//! to, never on an emulator internal. Every row is deployed into ONE emulator, as a client
//! deploys the two halves of a differential against one account.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, elements, form_encode, outputs_of};

/// The Region every request here is signed for — the one `wait_for_stack_status` polls in.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The stack status every row waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long an observation of a delivery waits before giving up on it.
const DELIVERY_BUDGET: Duration = Duration::from_secs(25);

/// How long the rows that must observe NOTHING wait before concluding nothing came.
const QUIET_BUDGET: Duration = Duration::from_secs(4);

/// How often either look.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The `Analytics` property `cdk synth` writes on the metadata resource.
const ANALYTICS: &str = "v2:deflate64:H4sIAAAAAAAA";

/// The parameter a synthesized template reads the bootstrap version from.
const BOOTSTRAP_VERSION_PARAMETER: &str = "/cdk-bootstrap/hnb659fds/version";

/// CloudFormation, SNS, Lambda, Amazon SQS and IAM over ONE engine, as
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
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call against `signing_name`, with whatever status it answered.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
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
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation call, with whatever status it answered.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "2010-05-15",
        action,
        members,
    )
    .await
}

/// One SNS call that must answer 200.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(
        router,
        "sns",
        &format!("sns.{REGION}.amazonaws.com"),
        "2010-03-31",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One Amazon SQS call (`awsJson1_0`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
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
        "case {case:?}: SQS {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The handler the stack declares inline: it forwards the message of every SNS record it is
/// handed to the queue whose URL its environment carries, which is where the delivery is
/// observed.
const FORWARDING_SOURCE: &str = r#"
import os

import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = os.environ["SINK_URL"]
    forwarded = 0
    for record in event["Records"]:
        client.send_message(QueueUrl=url, MessageBody=record["Sns"]["Message"])
        forwarded += 1
    return {"forwarded": forwarded}
"#;

/// What one row declares about the grant on the subscribed function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// The stack declares no `AWS::Lambda::Permission` at all.
    None,
    /// The stack grants `lambda:InvokeFunction` to `sns.amazonaws.com` for its own topic.
    ForThisTopic,
    /// The stack grants it to `sns.amazonaws.com`, but scoped to ANOTHER topic's ARN.
    ForAnotherTopic,
    /// The stack grants `lambda:InvokeFunction` for its own topic to another service's
    /// principal.
    ToAnotherPrincipal,
}

/// One deployment of the app: what it declares, and whether the publish must reach the
/// function.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    stack: &'static str,
    grant: Grant,
    /// The `Timeout` the function is declared with, or `None` to leave it at the default
    /// Lambda gives a function whose declaration states none.
    timeout: Option<u64>,
    /// Whether the published message must reach the sink the function forwards to.
    delivered: bool,
}

/// The rows: the same app, differing only in the grant the function carries.
const ROWS: &[Row] = &[
    Row {
        case: "no AWS::Lambda::Permission at all",
        stack: "sns-lambda-cdk-ungranted",
        grant: Grant::None,
        timeout: Some(30),
        delivered: false,
    },
    Row {
        case: "lambda:InvokeFunction for sns.amazonaws.com and this topic",
        stack: "sns-lambda-cdk-granted",
        grant: Grant::ForThisTopic,
        timeout: Some(30),
        delivered: true,
    },
    Row {
        case: "the grant scoped to another topic's ARN",
        stack: "sns-lambda-cdk-other-source",
        grant: Grant::ForAnotherTopic,
        timeout: Some(30),
        delivered: false,
    },
    Row {
        case: "the grant made to another service principal",
        stack: "sns-lambda-cdk-other-principal",
        grant: Grant::ToAnotherPrincipal,
        timeout: Some(30),
        delivered: false,
    },
    Row {
        case: "the grant present and no declared Timeout",
        stack: "sns-lambda-cdk-default-timeout",
        grant: Grant::ForThisTopic,
        timeout: None,
        delivered: true,
    },
];

/// The template one row deploys, in the shape `cdk synth` writes it: the bootstrap-version
/// parameter and the rule over it, the metadata resource and its condition, an execution role
/// and its inline policy, an UNNAMED function whose environment carries the sink's URL, the
/// topic, the `lambda`-protocol subscription and — per the row — the permission.
fn template(row: &Row, sink_arn: &str, sink_url: &str) -> String {
    let mut resources = json!({
        "ForwarderServiceRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Action": "sts:AssumeRole",
                        "Effect": "Allow",
                        "Principal": { "Service": "lambda.amazonaws.com" },
                    }],
                },
                "ManagedPolicyArns": [{"Fn::Join": ["", [
                    "arn:", {"Ref": "AWS::Partition"},
                    ":iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
                ]]}],
            },
            "Metadata": {"aws:cdk:path": format!("{}/Forwarder/ServiceRole", row.stack)},
        },
        "ForwarderServiceRoleDefaultPolicy": {
            "Type": "AWS::IAM::Policy",
            "Properties": {
                "PolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Action": ["sqs:SendMessage", "sqs:GetQueueAttributes"],
                        "Effect": "Allow",
                        "Resource": sink_arn,
                    }],
                },
                "PolicyName": "ForwarderServiceRoleDefaultPolicy",
                "Roles": [{"Ref": "ForwarderServiceRole"}],
            },
        },
        "Forwarder": {
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "Code": {"ZipFile": FORWARDING_SOURCE},
                "Environment": {"Variables": {"SINK_URL": sink_url}},
                "Handler": "index.lambda_handler",
                "Role": {"Fn::GetAtt": ["ForwarderServiceRole", "Arn"]},
                "Runtime": "python3.12",
            },
            "DependsOn": ["ForwarderServiceRoleDefaultPolicy", "ForwarderServiceRole"],
            "Metadata": {"aws:cdk:path": format!("{}/Forwarder/Resource", row.stack)},
        },
        "Topic": {
            "Type": "AWS::SNS::Topic",
            "Metadata": {"aws:cdk:path": format!("{}/Topic/Resource", row.stack)},
        },
        "TopicForwarderSubscription": {
            "Type": "AWS::SNS::Subscription",
            "Properties": {
                "Endpoint": {"Fn::GetAtt": ["Forwarder", "Arn"]},
                "Protocol": "lambda",
                "TopicArn": {"Ref": "Topic"},
            },
        },
        "CDKMetadata": {
            "Type": "AWS::CDK::Metadata",
            "Properties": {"Analytics": ANALYTICS},
            "Condition": "CDKMetadataAvailable",
        },
    });
    let permission = match row.grant {
        Grant::None => None,
        Grant::ForThisTopic => Some(json!({
            "Action": "lambda:InvokeFunction",
            "FunctionName": {"Fn::GetAtt": ["Forwarder", "Arn"]},
            "Principal": "sns.amazonaws.com",
            "SourceArn": {"Ref": "Topic"},
        })),
        Grant::ForAnotherTopic => Some(json!({
            "Action": "lambda:InvokeFunction",
            "FunctionName": {"Fn::GetAtt": ["Forwarder", "Arn"]},
            "Principal": "sns.amazonaws.com",
            "SourceArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:another-topic"),
        })),
        Grant::ToAnotherPrincipal => Some(json!({
            "Action": "lambda:InvokeFunction",
            "FunctionName": {"Fn::GetAtt": ["Forwarder", "Arn"]},
            "Principal": "events.amazonaws.com",
            "SourceArn": {"Ref": "Topic"},
        })),
    };
    if let Some(seconds) = row.timeout {
        resources["Forwarder"]["Properties"]["Timeout"] = json!(seconds);
    }
    if let Some(properties) = permission {
        resources["ForwarderAllowInvoke"] = json!({
            "Type": "AWS::Lambda::Permission",
            "Properties": properties,
        });
    }
    json!({
        "Parameters": {
            "BootstrapVersion": {
                "Type": "AWS::SSM::Parameter::Value<String>",
                "Default": BOOTSTRAP_VERSION_PARAMETER,
                "Description": "Version of the CDK Bootstrap resources in this environment",
            },
        },
        "Rules": {
            "CheckBootstrapVersion": {
                "Assertions": [{
                    "Assert": {"Fn::Not": [{"Fn::Contains": [
                        ["1", "2", "3", "4", "5"], {"Ref": "BootstrapVersion"},
                    ]}]},
                    "AssertDescription":
                        "CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' \
                         with a recent version of the CDK CLI.",
                }],
            },
        },
        "Conditions": {
            "CDKMetadataAvailable": {"Fn::Or": [
                {"Fn::Equals": [{"Ref": "AWS::Region"}, REGION]},
                {"Fn::Equals": [{"Ref": "AWS::Region"}, "eu-west-1"]},
            ]},
        },
        "Resources": resources,
        "Outputs": {
            "TopicArn": {"Value": {"Ref": "Topic"}},
            "FunctionName": {"Value": {"Ref": "Forwarder"}},
            "FunctionArn": {"Value": {"Fn::GetAtt": ["Forwarder", "Arn"]}},
        },
    })
    .to_string()
}

/// Every message body waiting on the queue at `url`.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
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

/// Wait for `marker` to appear on the queue at `url`, and report every body seen.
async fn wait_for(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    marker: &str,
) -> Vec<String> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        seen.extend(drain(router, case, url).await);
        if seen.iter().any(|body| body.contains(marker)) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Wait out [`QUIET_BUDGET`] and report every body the queue at `url` held in it.
async fn stayed_quiet(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let deadline = Instant::now() + QUIET_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        seen.extend(drain(router, case, url).await);
        if !seen.is_empty() {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    seen
}

/// Deploy one row's stack and answer its outputs.
async fn deploy(
    router: &MultiServiceRouter,
    row: &Row,
    sink_arn: &str,
    sink_url: &str,
) -> BTreeMap<String, String> {
    let case = row.case;
    let body = template(row, sink_arn, sink_url);
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", &body),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );
    let (reached, described) = wait_for_stack_status(router, row.stack, CREATE_COMPLETE).await;
    let failures = if reached == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) =
            cloudformation(router, "DescribeStackEvents", &[("StackName", row.stack)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
         resource failures {failures:?}"
    );
    outputs_of(&described)
}

/// Every row: one stack, one sink of its own, one publish, one observation.
#[tokio::test]
async fn a_synthesized_sns_lambda_subscription_invokes_the_function_only_where_sns_may() {
    let router = build_router();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;

        // The sink the handler forwards to, created OUTSIDE the stack — the queue a client
        // hands a deployment rather than one the deployment owns.
        let sink_name = format!("{}-sink", row.stack);
        let created = sqs(&router, case, "CreateQueue", &json!({ "QueueName": sink_name })).await;
        let sink_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateQueue answered {created}"))
            .to_string();
        let attributes = sqs(
            &router,
            case,
            "GetQueueAttributes",
            &json!({ "QueueUrl": sink_url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
        let sink_arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: GetQueueAttributes answered {attributes}"))
            .to_string();

        let outputs = deploy(&router, row, &sink_arn, &sink_url).await;
        let topic = outputs
            .get("TopicArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no TopicArn"));
        let function = outputs
            .get("FunctionName")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no FunctionName"));
        let function_arn = outputs
            .get("FunctionArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no FunctionArn"));

        // The subscription the stack declared is the one SNS holds, over the `lambda`
        // protocol and addressing the function the stack created.
        let listed = sns(
            &router,
            case,
            "ListSubscriptionsByTopic",
            &[("TopicArn", &topic)],
        )
        .await;
        assert_eq!(
            elements(&listed, "Protocol"),
            vec!["lambda".to_string()],
            "case {case:?}: expected one lambda subscription of {topic}, got {listed}"
        );
        assert_eq!(
            elements(&listed, "Endpoint"),
            vec![function_arn.clone()],
            "case {case:?}: expected the subscription to address {function_arn}, got {listed}"
        );

        // The function the stack published is one AWS Lambda takes calls on the moment the
        // stack reports itself complete — nothing is waited for here — and the handler it
        // deployed forwards an SNS event it is handed. So what the rows differ on is the
        // DELIVERY and nothing else.
        let probe = format!("probe-{index}");
        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            json!({"Records": [{"Sns": {"Message": probe}}]})
                .to_string()
                .as_bytes(),
        )
        .await;
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            (status, signal.clone(), payload.clone()),
            (200, None, json!({"forwarded": 1})),
            "case {case:?}: expected the deployed handler to forward one record, got {status} \
             / {signal:?} / {payload}"
        );
        let forwarded = wait_for(&router, case, &sink_url, &probe).await;
        assert!(
            forwarded.iter().any(|body| body == &probe),
            "case {case:?}: expected the handler's own forward of {probe:?} on the sink, got \
             {forwarded:?}"
        );

        // The publish, and what it reached.
        let marker = format!("marker-{index}");
        let published = sns(
            &router,
            case,
            "Publish",
            &[("TopicArn", &topic), ("Message", &marker)],
        )
        .await;
        assert!(
            !element(&published, "MessageId").is_empty(),
            "case {case:?}: the publish is accepted whether or not the delivery is \
             permitted, got {published}"
        );
        if row.delivered {
            let delivered = wait_for(&router, case, &sink_url, &marker).await;
            assert!(
                delivered.iter().any(|body| body.contains(&marker)),
                "case {case:?}: expected the subscribed function to be invoked and to forward \
                 {marker:?} to the sink queue, got {delivered:?}"
            );
        } else {
            let quiet = stayed_quiet(&router, case, &sink_url).await;
            assert!(
                quiet.is_empty(),
                "case {case:?}: expected the function never to be invoked, so nothing reaches \
                 the sink queue, got {quiet:?}"
            );
        }
    }
}
