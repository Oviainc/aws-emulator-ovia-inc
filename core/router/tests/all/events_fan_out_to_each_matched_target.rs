// SPDX-License-Identifier: CC0-1.0

//! One published event fanned out to SEVERAL targets, across the four services that
//! own the branches: EventBridge routes, Lambda is invoked, SNS is published to and
//! fans the notification out to its SQS subscriber, and SQS holds what arrived.
//!
//! The rule pinned here: every target of every rule an event matched is dispatched
//! INDEPENDENTLY — one target's outcome never decides another's — so each branch's
//! own effect is observable for a single `PutEvents`.
//!
//! The rows differ along what the rule generalises over: how many rules match, how
//! many targets each carries, which service each target names, and the order the
//! targets are written in.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::form_encode;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = "000000000000";

/// The `source` every event this suite publishes carries.
const SOURCE: &str = "fanout.src";

/// The bus every rule here lives on.
const BUS: &str = "fanout-bus";

/// A deployment package as `Code.ZipFile` carries it. Its content does not matter to
/// this suite: only that Lambda accepts the function and can be invoked by ARN.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// Long enough that a created function has left `Pending` for `Active`: "During this
/// time, you can't invoke or modify the function", and Lambda refuses an invocation
/// that arrives inside that window with `ResourceConflictException`.
const PAST_THE_FUNCTION_TRANSITION: Duration = Duration::from_millis(800);

/// The role a created function names.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/lambda-exec")
}

/// The four services this fan-out runs through, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// An SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SNS awsQuery call (API version 2010-03-31), as its status and XML body.
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One SNS call that must answer 200.
async fn sns_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = sns(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {body}"
    );
    body
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a queue, admit `sns.amazonaws.com` and `events.amazonaws.com` on it, and
/// hand back its url and ARN.
async fn create_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": ["sns.amazonaws.com", "events.amazonaws.com"] },
            "Action": "sqs:SendMessage",
            "Resource": arn
        }]
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    (url, arn)
}

/// Create a topic with an SQS subscriber, and hand back the topic's ARN.
async fn create_topic_subscribed_by(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queue_arn: &str,
) -> String {
    sns_ok(router, case, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = format!("arn:aws:sns:{REGION}:{ACCOUNT}:{name}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sns:Publish",
            "Resource": topic_arn
        }]
    });
    sns_ok(
        router,
        case,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("AttributeName", "Policy"),
            ("AttributeValue", policy.to_string().as_str()),
        ],
    )
    .await;
    sns_ok(
        router,
        case,
        "Subscribe",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    topic_arn
}

/// Create a function that `events.amazonaws.com` may invoke, and hand back its ARN.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": role_arn(),
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": ZIP_FILE },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string();
    let (status, answer) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        json!({
            "StatementId": "events-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "events.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} ({answer})"
    );
    arn
}

/// How many asynchronous invocations Lambda has queued for `function`, which is what an
/// accepted `Event` invocation — the one EventBridge makes for a function target —
/// persists: "A success response (202 Accepted) indicates that the request is queued
/// for invocation."
fn queued_invocations(engine: &Arc<engine::InMemoryEngine>, function: &str) -> usize {
    let state = engine
        .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map_or(0, |record| record.async_invocations.len())
}

/// What EventBridge recorded for each handover of a routed event: the target's ARN and
/// the outcome, rendered as `invoked`, `not-attempted` or the failure's code.
fn handovers(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, String)> {
    use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;
    let state = engine.state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    let store = state.lock();
    store
        .invocations
        .iter()
        .map(|invocation| {
            let outcome = match &invocation.outcome {
                InvocationOutcome::Invoked => "invoked".to_string(),
                InvocationOutcome::NotAttempted(reason) => format!("not-attempted: {reason}"),
                InvocationOutcome::Failed { code, .. } => format!("failed: {code}"),
            };
            (invocation.target_arn.clone(), outcome)
        })
        .collect()
}

/// Receive up to `max` messages from `url`, draining them.
async fn received(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    max: i64,
) -> Vec<String> {
    let answer = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": max }),
    )
    .await;
    answer["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the fan-out table.
struct FanOut {
    case: &'static str,
    /// The `detail-type` the row publishes, which is also what its rules match on.
    detail_type: &'static str,
    /// How the matched targets are spread over rules: one entry per rule, each listing
    /// the target kinds that rule carries in the order they are written.
    rules: &'static [&'static [Branch]],
}

/// The kind of target one row writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    /// A Lambda function target.
    Function,
    /// An SNS topic target, which fans the notification out to its SQS subscriber.
    Topic,
    /// An SQS queue target, reached directly.
    Queue,
}

/// A single `PutEvents` reaches EVERY target of EVERY rule it matched.
#[tokio::test]
async fn one_event_reaches_every_target_of_every_matched_rule() {
    let cases = [
        FanOut {
            case: "one rule carrying a function and a topic target",
            detail_type: "FanOutBoth",
            rules: &[&[Branch::Function, Branch::Topic]],
        },
        FanOut {
            case: "the same two branches in the opposite order",
            detail_type: "FanOutReversed",
            rules: &[&[Branch::Topic, Branch::Function]],
        },
        FanOut {
            case: "two rules, one target each",
            detail_type: "FanOutTwoRules",
            rules: &[&[Branch::Function], &[Branch::Topic]],
        },
        FanOut {
            case: "three targets of three services on one rule",
            detail_type: "FanOutThree",
            rules: &[&[Branch::Function, Branch::Topic, Branch::Queue]],
        },
        FanOut {
            case: "a single topic target",
            detail_type: "FanOutTopicOnly",
            rules: &[&[Branch::Topic]],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let (router, engine) = router();
        events(&router, case.case, "CreateEventBus", &json!({ "Name": BUS })).await;

        // The subscriber queue every topic branch of this row fans out into, and the
        // queue a direct queue target delivers to.
        let (subscriber_url, subscriber_arn) =
            create_queue(&router, case.case, &format!("fanout-sub-{index}")).await;
        let (direct_url, direct_arn) =
            create_queue(&router, case.case, &format!("fanout-direct-{index}")).await;
        let topic_arn = create_topic_subscribed_by(
            &router,
            case.case,
            &format!("fanout-topic-{index}"),
            &subscriber_arn,
        )
        .await;
        let function_name = format!("fanout-fn-{index}");
        let function_arn = create_function(&router, case.case, &function_name).await;

        let mut expected_targets: Vec<String> = Vec::new();
        let mut topics = 0usize;
        let mut queues = 0usize;
        let mut functions = 0usize;
        for (rule_index, targets) in case.rules.iter().enumerate() {
            let rule = format!("fanout-rule-{index}-{rule_index}");
            events(
                &router,
                case.case,
                "PutRule",
                &json!({
                    "Name": rule,
                    "EventBusName": BUS,
                    "EventPattern": json!({
                        "source": [SOURCE],
                        "detail-type": [case.detail_type],
                    })
                    .to_string(),
                }),
            )
            .await;
            let written: Vec<Value> = targets
                .iter()
                .enumerate()
                .map(|(target_index, branch)| {
                    let arn = match branch {
                        Branch::Function => {
                            functions += 1;
                            function_arn.clone()
                        }
                        Branch::Topic => {
                            topics += 1;
                            topic_arn.clone()
                        }
                        Branch::Queue => {
                            queues += 1;
                            direct_arn.clone()
                        }
                    };
                    expected_targets.push(arn.clone());
                    json!({ "Id": format!("t{target_index}"), "Arn": arn })
                })
                .collect();
            let targeted = events(
                &router,
                case.case,
                "PutTargets",
                &json!({ "Rule": rule, "EventBusName": BUS, "Targets": written }),
            )
            .await;
            assert_eq!(
                targeted["FailedEntryCount"].as_i64(),
                Some(0),
                "case {:?}: every target must be accepted, got {targeted}",
                case.case
            );
        }

        // Lambda refuses an invocation that arrives while the function is still being
        // provisioned, so the row waits the transition out before publishing: the
        // handover under test is the one AWS would make to an `Active` function.
        tokio::time::sleep(PAST_THE_FUNCTION_TRANSITION).await;

        let published = events(
            &router,
            case.case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": BUS,
                    "Source": SOURCE,
                    "DetailType": case.detail_type,
                    "Detail": json!({ "order": index }).to_string(),
                }]
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"].as_i64(),
            Some(0),
            "case {:?}: the event must be accepted, got {published}",
            case.case
        );

        // Every target of every matched rule was handed the event, and each handover
        // was recorded on its own.
        let recorded = handovers(&engine);
        let dispatched: Vec<String> = recorded
            .iter()
            .map(|(arn, _)| arn.clone())
            .collect();
        let mut sorted_dispatched = dispatched.clone();
        sorted_dispatched.sort();
        let mut sorted_expected = expected_targets.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_dispatched, sorted_expected,
            "case {:?}: every matched target must be dispatched, got {recorded:?}",
            case.case
        );
        for (arn, outcome) in &recorded {
            assert_eq!(
                outcome, "invoked",
                "case {:?}: the handover to {arn} must be invoked, got {recorded:?}",
                case.case
            );
        }

        // The Lambda branch's own far-end effect: EventBridge invokes a function target
        // asynchronously, and an accepted asynchronous invocation is queued against the
        // function — one per function target of every matched rule.
        assert_eq!(
            queued_invocations(&engine, &function_name),
            functions,
            "case {:?}: expected {functions} asynchronous invocation(s) queued for \
             {function_name}",
            case.case
        );

        // The SNS branch's own far-end effect: one notification per topic target in
        // the subscriber queue, carrying the published event envelope.
        let bodies = received(&router, case.case, &subscriber_url, 10).await;
        assert_eq!(
            bodies.len(),
            topics,
            "case {:?}: expected {topics} notification(s) in the subscriber queue, got {bodies:?}",
            case.case
        );
        for body in &bodies {
            let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!(
                    "case {:?}: the subscriber receives an SNS notification document, got \
                     {body:?} ({error})",
                    case.case
                )
            });
            let message = envelope["Message"].as_str().unwrap_or_default().to_string();
            let event: Value = serde_json::from_str(&message).unwrap_or_else(|error| {
                panic!(
                    "case {:?}: the notification's Message is the event envelope, got \
                     {message:?} ({error})",
                    case.case
                )
            });
            assert_eq!(
                event["detail-type"].as_str(),
                Some(case.detail_type),
                "case {:?}: the published detail-type must survive the branch, got {event}",
                case.case
            );
            assert_eq!(
                event["detail"]["order"].as_i64(),
                Some(index as i64),
                "case {:?}: the published detail must survive the branch, got {event}",
                case.case
            );
        }

        // The direct queue branch, independent of the other two.
        let direct = received(&router, case.case, &direct_url, 10).await;
        assert_eq!(
            direct.len(),
            queues,
            "case {:?}: expected {queues} message(s) in the directly targeted queue \
             {direct_arn}, got {direct:?}",
            case.case
        );
    }
}

/// The same fan-out on the account's DEFAULT bus, which a rule written with no
/// `EventBusName` belongs to: the bus an event is routed on is not what decides whether
/// each of a rule's targets is dispatched.
///
/// Both branches are driven by ONE `PutEvents`, and the subscriber queue admits only
/// `sns.amazonaws.com` — the publishing principal of the branch that reaches it — so
/// the message that arrives is the SNS fan-out's own rather than a direct delivery.
#[tokio::test]
async fn the_default_bus_fans_one_event_out_to_both_a_function_and_a_topic() {
    let case = "a function and a topic target of one rule on the default bus";
    let (router, engine) = router();

    let created = sqs(
        &router,
        case,
        "CreateQueue",
        &json!({ "QueueName": "default-bus-sub" }),
    )
    .await;
    let subscriber_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let subscriber_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:default-bus-sub");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": subscriber_arn
        }]
    });
    sqs(
        &router,
        case,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": subscriber_url,
            "Attributes": { "Policy": policy.to_string() }
        }),
    )
    .await;
    let topic_arn =
        create_topic_subscribed_by(&router, case, "default-bus-topic", &subscriber_arn).await;
    let function_name = "default-bus-fn";
    let function_arn = create_function(&router, case, function_name).await;

    events(
        &router,
        case,
        "PutRule",
        &json!({
            "Name": "default-bus-fanout",
            "EventPattern": json!({ "source": [SOURCE] }).to_string(),
        }),
    )
    .await;
    let targeted = events(
        &router,
        case,
        "PutTargets",
        &json!({
            "Rule": "default-bus-fanout",
            "Targets": [
                { "Id": "t1", "Arn": function_arn },
                { "Id": "t2", "Arn": topic_arn }
            ]
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: both targets must be accepted, got {targeted}"
    );

    tokio::time::sleep(PAST_THE_FUNCTION_TRANSITION).await;
    let published = events(
        &router,
        case,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": SOURCE,
                "DetailType": "DefaultBusFanOut",
                "Detail": json!({ "branch": "both" }).to_string(),
            }]
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: the event must be accepted, got {published}"
    );

    let recorded = handovers(&engine);
    assert_eq!(
        recorded.len(),
        2,
        "case {case:?}: both targets must be handed the event, got {recorded:?}"
    );
    for (arn, outcome) in &recorded {
        assert_eq!(
            outcome, "invoked",
            "case {case:?}: the handover to {arn} must be invoked, got {recorded:?}"
        );
    }
    assert_eq!(
        queued_invocations(&engine, function_name),
        1,
        "case {case:?}: the function branch must queue one asynchronous invocation"
    );
    let bodies = received(&router, case, &subscriber_url, 10).await;
    assert_eq!(
        bodies.len(),
        1,
        "case {case:?}: the SNS branch must reach the subscriber queue, got {bodies:?}"
    );
}
