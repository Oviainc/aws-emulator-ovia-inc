// SPDX-License-Identifier: CC0-1.0

//! What a client SEES of a delivery EventBridge and Amazon SNS make to a target whose own
//! resource-based policy decides it: the invoked function's handler really runs and its line
//! is readable in that function's CloudWatch Logs log group, and the published topic's
//! subscriber really holds the notification.
//!
//! The rule pinned here: **a delivery to a target whose invocation AWS governs by the
//! TARGET's own resource-based policy — a Lambda function, an Amazon SNS topic
//! ("for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
//! resource-based policies", `PutTargets`) — is made as the delivering service's own service
//! principal and happens only where that target's document admits THAT principal THAT
//! target's invoking action (`lambda:InvokeFunction` for a function, `sns:Publish` for a
//! topic); every target of a matched rule, and every subscription of a topic, is decided on
//! its own, so one target's refusal never withholds another's delivery; and a delivery that
//! was made is observable at the far end — a function's own output in the log group it logs
//! to ("by default, Lambda functions send logs to a default log group named
//! `/aws/lambda/<function name>`"), a topic's notification on its subscriber.**
//!
//! Six crates have to agree for that to hold (CloudFormation archives the inline handler into
//! the function's deployment package, EventBridge matches and hands over, Lambda decides the
//! invocation against the function's policy and then RUNS the handler, Amazon SNS decides the
//! publish against the topic's policy and fans it out, CloudWatch Logs holds what the handler
//! wrote, Amazon SQS holds what arrived, and IAM answers for the execution role the log
//! delivery is judged against), so the suite lives here rather than in any one crate. Every
//! observation is the one a client makes: the function's log group and the subscriber queue,
//! never an emulator internal.
//!
//! The rows differ along exactly what the rule generalises over: WHICH of the two targets'
//! documents admits the delivering principal, whether a document names the right action at
//! all, and which service makes the delivery (EventBridge's fan-out, and Amazon SNS's own
//! `lambda`-protocol subscription). `crate::events_fan_out_to_each_matched_target` is the
//! suite for the MULTIPLICITY of a fan-out with every grant in place, and
//! `crate::cloudformation_events_rule_lambda_target_permission_boundary` for what an
//! undeliverable handover is dead-lettered as; this one is the suite for the marker a client
//! polls the invoked function's logs for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{
    cloudformation, iam, sns, sqs, trust_policy, ACCOUNT, CREATE_COMPLETE, LOOK_INTERVAL,
};

/// The Region every request here is signed for — the one the shared stack reader
/// ([`wait_for_stack_status`]) and the shared awsQuery helpers sign for.
const REGION: &str = "us-east-1";

/// The `source` and `detail-type` every rule here matches on.
const SOURCE: &str = "events.sns.lambda.observed";
const DETAIL_TYPE: &str = "Delivery Marker";

/// The service principals the two delivering services hand a resource-policy-governed target
/// over as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The action an invocation of a function performs, and the one a publish onto a topic
/// performs: what a grant for either delivery has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";
const PUBLISH_ACTION: &str = "sns:Publish";

/// An action of each of those services that is NOT the one the delivery performs, which a row
/// grants instead to show that it is the ACTION and not merely a statement that decides the
/// delivery.
const OTHER_LAMBDA_ACTION: &str = "lambda:GetFunction";
const OTHER_SNS_ACTION: &str = "sns:Subscribe";

/// The execution role every declared function is created with, provisioned in IAM before the
/// stack so the template names a role that service really holds.
const ROLE_NAME: &str = "events-sns-lambda-observed-exec";

/// How long an observation that must SEE something waits for it, how long one that must see
/// NOTHING waits before concluding nothing came, and how many times a function's state is
/// polled before the suite gives up on it.
const DELIVERY_BUDGET: Duration = Duration::from_secs(30);
const QUIET_BUDGET: Duration = Duration::from_secs(5);
const POLLS: usize = 600;

/// The handler every row deploys inline: it logs the whole event it was handed through the
/// logging facility — the shape AWS documents for a Python handler — so the per-run marker the
/// delivery carried is readable in the function's log group whichever service delivered it.
const LOGGING_SOURCE: &str = r#"
import json
import logging

LOGGER = logging.getLogger()
LOGGER.setLevel(logging.INFO)


def lambda_handler(event, context):
    LOGGER.info("handled %s", json.dumps(event))
    return {"handled": True}
"#;

/// The whole emulator as it is ASSEMBLED, on one engine: the log delivery the function's own
/// run makes is dispatched back into this same router, so the group this suite reads is the
/// group that run wrote.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
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

/// One `X-Amz-Target` RPC call of `signing_name`, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    json_version: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", target)
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "events",
        &format!("AWSEvents.{operation}"),
        "1.1",
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`), with whatever status
/// it answered: a group no delivery has created yet is a refusal this suite reads THROUGH
/// rather than asserts on.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        router,
        "logs",
        &format!("Logs_20140328.{operation}"),
        "1.1",
        body,
    )
    .await
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create the execution role every template here names, where IAM holds it: trusted by
/// Lambda, and granting the CloudWatch Logs calls a run's own log delivery makes — a role
/// short of them leaves nothing for a client to read, which is not the boundary under test.
async fn execution_role(router: &MultiServiceRouter) {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "write-its-own-logs"),
            ("PolicyDocument", &policy),
        ],
    )
    .await;
}

/// Deploy a stack holding ONE function with the inline handler above, and answer its
/// `(name, arn)`.
///
/// The function comes from a template because that is what archives inline source into a
/// deployment package this emulator can run: `Code.ZipFile` in a template "is the function's
/// SOURCE TEXT" while the API's member is the archive's bytes. Everything the rule is about —
/// the rule, its targets, the subscription and the grants — is then wired through each
/// service's OWN API below, the way the client under test wires it.
async fn deployed_function(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> (String, String) {
    let name = format!("{stack}-handler");
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Handler": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": name,
                    "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Timeout": 10,
                    "Code": { "ZipFile": LOGGING_SOURCE },
                },
            },
        },
        "Outputs": {
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Handler", "Arn"] } },
        },
    })
    .to_string();
    cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    let (status, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {case:?}: expected the handler stack to deploy, got {status} with {described}"
    );
    let arn = outputs_of(&described)
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to publish a FunctionArn, got {described}")
        });

    // The function has to be invocable before anything is delivered to it: "During this time,
    // you can't invoke or modify the function" (`CreateFunction`), so a handover withheld here
    // would be the lifecycle's refusal rather than the policy's.
    for _ in 0..POLLS {
        let (_, reported) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return (name, arn);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: Lambda never reported {name} Active");
}

/// Add one statement to the function's resource-based policy, granting `principal` `action`.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    statement_id: &str,
    principal: &str,
    action: &str,
) {
    let (status, answered) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        Some(&json!({
            "StatementId": statement_id,
            "Action": action,
            "Principal": principal,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission({principal}, {action}), got \
         {status} with {answered}"
    );
}

/// Create a queue and admit `sns.amazonaws.com` on it, and answer its `(url, arn)`.
async fn subscriber_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SNS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    })
    .to_string();
    sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    (url, arn)
}

/// Create a topic, subscribe `queue_arn` to it over `sqs`, and set its access policy to
/// `granted_action` for `events.amazonaws.com` when a row names one. Answer the topic's ARN.
async fn topic_subscribed_by(
    router: &MultiServiceRouter,
    name: &str,
    queue_arn: &str,
    granted_action: Option<&str>,
) -> String {
    let created = sns(router, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = element(&created, "TopicArn");
    if let Some(action) = granted_action {
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": EVENTS_PRINCIPAL },
                "Action": action,
                "Resource": topic_arn,
            }],
        })
        .to_string();
        sns(
            router,
            "SetTopicAttributes",
            &[
                ("TopicArn", &topic_arn),
                ("AttributeName", "Policy"),
                ("AttributeValue", &policy),
            ],
        )
        .await;
    }
    sns(
        router,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    topic_arn
}

/// Every message the log group `group` holds right now, as a client reads them. A group no
/// delivery has created is no messages rather than a failure: that is exactly what a client
/// polling for a marker that never came sees.
async fn log_messages(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (status, answered) =
        logs(router, "FilterLogEvents", &json!({ "logGroupName": group })).await;
    if status != 200 {
        return Vec::new();
    }
    answered["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| event["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Poll the function's log group for a line carrying `marker`, and answer every message it
/// held. A row that EXPECTS the line waits [`DELIVERY_BUDGET`] for it; one that expects none
/// waits out [`QUIET_BUDGET`], well past the span a delivered row takes to leave its line.
async fn awaited_log_messages(
    router: &MultiServiceRouter,
    function: &str,
    marker: &str,
    expected: bool,
) -> Vec<String> {
    let group = format!("/aws/lambda/{function}");
    let deadline = Instant::now() + if expected { DELIVERY_BUDGET } else { QUIET_BUDGET };
    loop {
        let seen = log_messages(router, &group).await;
        let found = seen.iter().any(|line| line.contains(marker));
        if (expected && found) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Every body waiting on the queue at `url`.
async fn drained(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let received = sqs(
        router,
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

/// Poll the subscriber queue for a body carrying `marker`, under the same two budgets.
async fn awaited_bodies(
    router: &MultiServiceRouter,
    url: &str,
    marker: &str,
    expected: bool,
) -> Vec<String> {
    let deadline = Instant::now() + if expected { DELIVERY_BUDGET } else { QUIET_BUDGET };
    let mut seen: Vec<String> = Vec::new();
    loop {
        seen.extend(drained(router, url).await);
        let found = seen.iter().any(|body| body.contains(marker));
        if (expected && found) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// One row of the fan-out table: which document admits the delivering principal, and what
/// each of the two branches therefore comes to.
struct FanOut {
    case: &'static str,
    stack: &'static str,
    /// The action the FUNCTION's policy grants `events.amazonaws.com`, or none for a function
    /// carrying no statement at all.
    function_grant: Option<&'static str>,
    /// The action the TOPIC's policy grants that same principal, or none for a topic carrying
    /// no access policy of its own.
    topic_grant: Option<&'static str>,
    /// Whether the function is therefore invoked, so its handler's line reaches its log group.
    invoked: bool,
    /// Whether the topic is therefore published to, so its subscriber holds the notification.
    published: bool,
}

/// Every row of the fan-out: each target of the one matched rule is delivered to exactly where
/// its own document admits this service's principal for its own action, and the two branches
/// decide each other nothing.
#[tokio::test]
async fn each_rule_target_is_delivered_to_only_where_its_own_policy_admits_eventbridge() {
    let rows = [
        FanOut {
            case: "both documents grant this service the delivery",
            stack: "events-observed-both",
            function_grant: Some(INVOKE_ACTION),
            topic_grant: Some(PUBLISH_ACTION),
            invoked: true,
            published: true,
        },
        FanOut {
            case: "only the function's policy grants it",
            stack: "events-observed-function",
            function_grant: Some(INVOKE_ACTION),
            topic_grant: None,
            invoked: true,
            published: false,
        },
        FanOut {
            case: "only the topic's policy grants it",
            stack: "events-observed-topic",
            function_grant: None,
            topic_grant: Some(PUBLISH_ACTION),
            invoked: false,
            published: true,
        },
        FanOut {
            case: "neither document grants it",
            stack: "events-observed-neither",
            function_grant: None,
            topic_grant: None,
            invoked: false,
            published: false,
        },
        FanOut {
            case: "the topic's policy grants another action of its own service",
            stack: "events-observed-other-action",
            function_grant: Some(INVOKE_ACTION),
            topic_grant: Some(OTHER_SNS_ACTION),
            invoked: true,
            published: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = build_router();
        execution_role(&router).await;
        let (function, function_arn) = deployed_function(&router, case, row.stack).await;
        if let Some(action) = row.function_grant {
            add_permission(
                &router,
                case,
                &function,
                "events-invoke",
                EVENTS_PRINCIPAL,
                action,
            )
            .await;
        }
        let (queue_url, queue_arn) =
            subscriber_queue(&router, case, &format!("{}-sub", row.stack)).await;
        let topic_arn = topic_subscribed_by(
            &router,
            &format!("{}-topic", row.stack),
            &queue_arn,
            row.topic_grant,
        )
        .await;

        let rule = format!("{}-rule", row.stack);
        events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule,
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [DETAIL_TYPE],
                })
                .to_string(),
            }),
        )
        .await;
        let targeted = events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [
                    { "Id": "function", "Arn": function_arn },
                    { "Id": "topic", "Arn": topic_arn },
                ],
            }),
        )
        .await;
        assert_eq!(
            targeted["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: expected both targets to be accepted, got {targeted}"
        );

        let marker = format!("{}-marker", row.stack);
        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({ "marker": marker }).to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: expected the entry to be accepted whatever the deliveries come to, \
             got {published}"
        );

        // The function branch, read where a client reads it: the group the function logs to.
        let lines = awaited_log_messages(&router, &function, &marker, row.invoked).await;
        let logged = lines.iter().any(|line| line.contains(&marker));
        assert_eq!(
            logged, row.invoked,
            "case {case:?}: expected the function's log group to hold a line carrying \
             {marker:?} = {}, got {logged} with {lines:?}",
            row.invoked
        );

        // The topic branch, decided on its own document and observed on its subscriber.
        let bodies = awaited_bodies(&router, &queue_url, &marker, row.published).await;
        let arrived = bodies.iter().any(|body| body.contains(&marker));
        assert_eq!(
            arrived, row.published,
            "case {case:?}: expected the topic's subscriber to hold a notification carrying \
             {marker:?} = {}, got {arrived} with {bodies:?}",
            row.published
        );
    }
}

/// One row of the subscription table: what the function's policy grants Amazon SNS, and
/// whether a publish therefore runs the subscribed function.
struct Subscription {
    case: &'static str,
    stack: &'static str,
    /// The action the function's policy grants `sns.amazonaws.com`, or none for a function
    /// carrying no statement at all.
    grant: Option<&'static str>,
    invoked: bool,
}

/// Every row of the subscription: a publish onto a topic with a `lambda`-protocol subscription
/// runs the subscribed function exactly where that FUNCTION's own document admits Amazon SNS
/// the invocation — the same rule the rule targets above are decided by, with the other
/// delivering service's principal.
#[tokio::test]
async fn a_lambda_subscription_runs_its_function_only_where_its_own_policy_admits_sns() {
    let rows = [
        Subscription {
            case: "the function's policy grants Amazon SNS the invocation",
            stack: "sns-observed-granted",
            grant: Some(INVOKE_ACTION),
            invoked: true,
        },
        Subscription {
            case: "the function's policy grants Amazon SNS another action of that service",
            stack: "sns-observed-other-action",
            grant: Some(OTHER_LAMBDA_ACTION),
            invoked: false,
        },
        Subscription {
            case: "the function carries no statement at all",
            stack: "sns-observed-ungranted",
            grant: None,
            invoked: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = build_router();
        execution_role(&router).await;
        let (function, function_arn) = deployed_function(&router, case, row.stack).await;
        if let Some(action) = row.grant {
            add_permission(
                &router,
                case,
                &function,
                "sns-invoke",
                SNS_PRINCIPAL,
                action,
            )
            .await;
        }

        let topic_name = format!("{}-topic", row.stack);
        let created = sns(&router, "CreateTopic", &[("Name", &topic_name)]).await;
        let topic_arn = element(&created, "TopicArn");
        let subscribed = sns(
            &router,
            "Subscribe",
            &[
                ("TopicArn", &topic_arn),
                ("Protocol", "lambda"),
                ("Endpoint", &function_arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
        assert!(
            !element(&subscribed, "SubscriptionArn").is_empty(),
            "case {case:?}: expected the lambda subscription to be created, got {subscribed}"
        );

        let marker = format!("{}-marker", row.stack);
        let answered = sns(
            &router,
            "Publish",
            &[("TopicArn", &topic_arn), ("Message", &marker)],
        )
        .await;
        assert!(
            !element(&answered, "MessageId").is_empty(),
            "case {case:?}: expected the publish to be accepted whether or not the delivery is \
             permitted, got {answered}"
        );

        let lines = awaited_log_messages(&router, &function, &marker, row.invoked).await;
        let logged = lines.iter().any(|line| line.contains(&marker));
        assert_eq!(
            logged, row.invoked,
            "case {case:?}: expected the function's log group to hold a line carrying \
             {marker:?} = {}, got {logged} with {lines:?}",
            row.invoked
        );
    }
}
