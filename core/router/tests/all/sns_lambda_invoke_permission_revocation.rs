// SPDX-License-Identifier: CC0-1.0

//! Revoking ONE subscribed function's invoke permission stops that function's deliveries and
//! leaves its granted twin's deliveries alone — the grant's LIFECYCLE through Lambda's own
//! control plane, over two functions subscribed to one topic.
//!
//! The rule pinned here: **SNS delivers each published message to every confirmed
//! `lambda`-protocol subscription of the topic by invoking the endpoint function
//! asynchronously ("lambda – delivery of JSON-encoded message to an Lambda function",
//! `SubscribeInput$Protocol`), as the service principal `sns.amazonaws.com`, so each delivery
//! is admitted only by THAT function's own resource-based policy: the policy is per function
//! and is made of the statements `AddPermission` added and `RemovePermission` took away
//! (`GetPolicy` reports "the resource-based IAM policy for a function"), so the grant each
//! publish finds is the one in force at that moment, and adding or removing a statement on one
//! function changes that function's deliveries and no other's.**
//!
//! The axis this suite adds to its siblings — which read the same rule off a stack that
//! declares the `AWS::Lambda::Permission` or does not
//! ([`crate::sns_lambda_subscription_pipeline`],
//! [`crate::sns_lambda_subscription_permission_boundary`]) — is the grant as a client CHANGES
//! it after the wiring is already in place, and over TWO subscribers of one topic rather than
//! one: the rounds below revoke a grant, restore it, and revoke the other function's instead,
//! so a delivery path that is simply dead and one that is correctly denied are told apart by
//! the twin that must keep firing in the very same round.
//!
//! Three crates have to agree for that to hold (SNS fans the publish out, Lambda holds each
//! policy and runs each handler, Amazon SQS holds what the handlers wrote), so the suite lives
//! here. Its router, its execution role and its awsQuery and Amazon SQS helpers are the
//! sibling boundary suite's, so there is ONE harness for the readings of this rule rather than
//! a copy per reading; what this file owns is the Lambda control-plane calls that move the
//! grant, and the rounds they move it through. Every observation is made where a client makes
//! it: on the sink queue the invoked function forwards to, and on `GetPolicy`.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{
    build_router, cloudformation, drain, execution_role, sns, sqs, ACCOUNT, CREATE_COMPLETE,
    DELIVERY_BUDGET, LOOK_INTERVAL, ROLE_NAME,
};

/// The Region every request here is signed for: the one the shared harness signs for, so the
/// resources this suite reaches through it are the ones it creates.
const REGION: &str = "us-east-1";

/// The stack that holds the two subscribed functions, and the queue both forward to.
const STACK: &str = "sns-revoke-twins";
const SINK_QUEUE: &str = "sns-revoke-twins-sink";

/// The two functions subscribed to the one topic, by the names the stack gives them and the
/// label each one stamps on what it forwards.
const ALPHA_FUNCTION: &str = "sns-revoke-twins-alpha";
const BETA_FUNCTION: &str = "sns-revoke-twins-beta";
const ALPHA_LABEL: &str = "alpha";
const BETA_LABEL: &str = "beta";

/// The statement id each function's grant is added and removed under: a client reconciling a
/// grant addresses the statement by id, which is what `RemovePermission` takes.
const STATEMENT_ID: &str = "sns-invoke";

/// The service principal SNS performs a delivery to a subscribed function as, and the action
/// that delivery performs on the function.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The error code Lambda answers a `GetPolicy` of a function holding no policy, which is what
/// a function whose last statement was removed holds.
const POLICY_ABSENT_CODE: &str = "ResourceNotFoundException";

/// How long a round that must observe NOTHING keeps looking before concluding nothing came —
/// well past the span an admitted delivery of the same publish takes in the rounds that do
/// observe one.
const QUIET_BUDGET: Duration = Duration::from_secs(5);

/// How long every round waits after the sink first holds what it expects, before the forwards
/// are counted: a delivery that happened twice lands inside it.
const SETTLE_SLACK: Duration = Duration::from_secs(1);

/// What separates a forward's label from the message it carries, so one body says both which
/// function ran and which publish it ran for.
const LABEL_SEPARATOR: char = '|';

/// The handler both functions deploy: it forwards every SNS record it is handed to the sink
/// queue, stamped with its own label. Both the label and the queue's name come from the
/// environment, so ONE source is deployed twice and the two functions differ only in what the
/// stack passes them.
const FORWARDING_SOURCE: &str = r#"
import os

import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName=os.environ["SINK_QUEUE"])["QueueUrl"]
    label = os.environ["LABEL"]
    forwarded = 0
    for record in event["Records"]:
        client.send_message(
            QueueUrl=url,
            MessageBody=label + "|" + record["Sns"]["Message"],
        )
        forwarded += 1
    return {"forwarded": forwarded}
"#;

/// A SigV4 `Authorization` whose credential scope names Lambda, which is what routes the
/// request. The signature is never checked: this router runs with request authentication off.
fn lambda_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/lambda/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// What one Lambda call answered: its status, its body as JSON, and the wire error code of a
/// refusal (`restJson1` carries it in `__type` or in the `x-amzn-errortype` header).
struct Answered {
    status: u16,
    body: Value,
    error_code: String,
}

/// One Lambda `restJson1` call.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answered {
    let payload = match &body {
        Some(body) => Bytes::from(body.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", lambda_authorization())
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let from_header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(from_header)
        .unwrap_or_default();
    Answered {
        status,
        body: parsed,
        error_code,
    }
}

/// One function resource of the stack: the forwarding source above, with the label and the
/// sink's name it is deployed with.
fn function_resource(name: &str, label: &str) -> Value {
    json!({
        "Type": "AWS::Lambda::Function",
        "Properties": {
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
            "Runtime": "python3.12",
            "Handler": "index.lambda_handler",
            "Timeout": 10,
            "Environment": {
                "Variables": { "LABEL": label, "SINK_QUEUE": SINK_QUEUE },
            },
            "Code": { "ZipFile": FORWARDING_SOURCE },
        },
    })
}

/// The stack this suite deploys: the two functions only. The topic and the two subscriptions
/// are made through the SNS API afterwards, and the grants through Lambda's — the calls a
/// client reconciling a fan-out's permissions makes.
fn template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Alpha": function_resource(ALPHA_FUNCTION, ALPHA_LABEL),
            "Beta": function_resource(BETA_FUNCTION, BETA_LABEL),
        },
        "Outputs": {
            "AlphaArn": { "Value": { "Fn::GetAtt": ["Alpha", "Arn"] } },
            "BetaArn": { "Value": { "Fn::GetAtt": ["Beta", "Arn"] } },
        },
    })
    .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active` — the
/// state an invocation needs — and answer the state it reported then. The span a created
/// function spends coming up is that service's own, so nothing here sleeps a fixed span on its
/// behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> String {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut last;
    loop {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        last = reported.body["State"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if last == "Active" || Instant::now() >= deadline {
            return last;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Grant this service the invocation of `function` through `AddPermission`, scoped by
/// `aws:SourceArn` to `source` when the caller names one.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    source: Option<&str>,
) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": INVOKE_ACTION,
        "Principal": SNS_PRINCIPAL,
    });
    if let Some(source) = source {
        body["SourceArn"] = json!(source);
    }
    let answered = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        Some(body),
    )
    .await;
    assert_eq!(
        answered.status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission on {function}, got {} with {}",
        answered.status, answered.body
    );
}

/// Take that grant away again through `RemovePermission`, which is the call the recorded
/// scenario revokes one twin's delivery with.
async fn remove_permission(router: &MultiServiceRouter, case: &str, function: &str) {
    let answered = lambda(
        router,
        "DELETE",
        &format!("/2015-03-31/functions/{function}/policy/{STATEMENT_ID}"),
        None,
    )
    .await;
    assert_eq!(
        answered.status, 204,
        "case {case:?}: expected HTTP 204 from RemovePermission on {function}, got {} with {}",
        answered.status, answered.body
    );
}

/// The statement ids `GetPolicy` reports for `function`, and `None` when it reports no policy
/// at all — the answer a function whose last statement was removed earns.
async fn policy_statements(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) -> Option<Vec<String>> {
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{function}/policy"),
        None,
    )
    .await;
    if answered.status == 404 {
        assert!(
            answered.error_code.contains(POLICY_ABSENT_CODE),
            "case {case:?}: expected GetPolicy of {function} to refuse with \
             {POLICY_ABSENT_CODE}, got {:?} with {}",
            answered.error_code,
            answered.body
        );
        return None;
    }
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 or 404 from GetPolicy of {function}, got {} with {}",
        answered.status, answered.body
    );
    let document: Value = answered.body["Policy"]
        .as_str()
        .and_then(|policy| serde_json::from_str(policy).ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: GetPolicy of {function} must report a policy document, got {}",
                answered.body
            )
        });
    Some(
        document["Statement"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|statement| statement["Sid"].as_str().unwrap_or_default().to_string())
            .collect(),
    )
}

/// What one function's grant is set to for a round, and what the round therefore expects of
/// it.
#[derive(Debug, Clone, Copy)]
struct Grant {
    /// Whether the function's policy grants this service the invocation when the round
    /// publishes.
    granted: bool,
    /// Whether the statement it is granted by is scoped by `aws:SourceArn` to the publishing
    /// topic. The two forms both admit the delivery, so the rounds carry one of each: a
    /// revocation that only ever worked on an unscoped statement is not a revocation.
    scoped: bool,
}

/// One round: the grant each of the two functions carries when the round publishes.
struct Round {
    case: &'static str,
    alpha: Grant,
    beta: Grant,
}

/// The rounds, in order, over the one topic and the one pair of subscriptions: the grant state
/// each round publishes under, reached from the round before it by the `AddPermission` or
/// `RemovePermission` the difference calls for. They differ along what the rule generalises
/// over — which of the two functions is granted, whether the statement is scoped to the topic,
/// and whether a grant is being added for the first time, taken away, or restored after a
/// revocation.
const ROUNDS: &[Round] = &[
    Round {
        case: "neither subscribed function has granted this service the invocation",
        alpha: REVOKED,
        beta: REVOKED,
    },
    Round {
        case: "both functions grant it, one unscoped and one scoped to the topic",
        alpha: GRANTED,
        beta: GRANTED_FOR_THE_TOPIC,
    },
    Round {
        case: "the unscoped grant is revoked and the scoped twin keeps its own",
        alpha: REVOKED,
        beta: GRANTED_FOR_THE_TOPIC,
    },
    Round {
        case: "the revoked grant is restored and the twin's scoped grant is revoked instead",
        alpha: GRANTED_FOR_THE_TOPIC,
        beta: REVOKED,
    },
    Round {
        case: "both grants are in force again",
        alpha: GRANTED,
        beta: GRANTED_FOR_THE_TOPIC,
    },
];

/// The three grant states a round sets a function to: no statement at all, the statement
/// unscoped, and the statement scoped by `aws:SourceArn` to the publishing topic.
const REVOKED: Grant = Grant {
    granted: false,
    scoped: false,
};
const GRANTED: Grant = Grant {
    granted: true,
    scoped: false,
};
const GRANTED_FOR_THE_TOPIC: Grant = Grant {
    granted: true,
    scoped: true,
};

/// Bring `function`'s policy to `wanted` from `held`, and report the grant it now holds.
///
/// A grant whose SCOPE changed is replaced rather than left as it was, because a statement is
/// addressed by its id: the id is already taken, so the new scope only reaches the policy by
/// removing the statement and adding it again.
async fn reconcile(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    topic: &str,
    held: Grant,
    wanted: Grant,
) -> Grant {
    if held.granted && (!wanted.granted || held.scoped != wanted.scoped) {
        remove_permission(router, case, function).await;
    }
    let absent = !held.granted || held.scoped != wanted.scoped;
    if wanted.granted && absent {
        let source = wanted.scoped.then_some(topic);
        add_permission(router, case, function, source).await;
    }
    wanted
}

/// Every body the sink queue holds that carries `marker`, polled until there are `expected` of
/// them or the round's budget runs out, then once more after a settling span so a SECOND
/// delivery of one publish is counted too.
///
/// `seen` accumulates every body the queue ever handed out, across rounds: a delivery that
/// arrives late — after the round that published it has already been asserted — is still in
/// there for the final reading to catch.
async fn forwards_of(
    router: &MultiServiceRouter,
    sink: &str,
    seen: &mut Vec<String>,
    marker: &str,
    expected: usize,
) -> Vec<String> {
    let budget = if expected == 0 {
        QUIET_BUDGET
    } else {
        DELIVERY_BUDGET
    };
    let deadline = Instant::now() + budget;
    loop {
        seen.extend(drain(router, sink).await);
        let held = carrying(seen, marker);
        if (expected > 0 && held.len() >= expected) || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    tokio::time::sleep(SETTLE_SLACK).await;
    seen.extend(drain(router, sink).await);
    carrying(seen, marker)
}

/// The labels of the forwards among `seen` that carry `marker`, sorted so a round's reading is
/// compared as a multiset rather than in arrival order.
fn carrying(seen: &[String], marker: &str) -> Vec<String> {
    let mut labels: Vec<String> = seen
        .iter()
        .filter_map(|body| body.split_once(LABEL_SEPARATOR))
        .filter(|(_, message)| *message == marker)
        .map(|(label, _)| label.to_string())
        .collect();
    labels.sort();
    labels
}

/// Every round, over one topic and one pair of subscriptions: each publish reaches exactly the
/// functions whose own policy grants this service the invocation at that moment.
#[tokio::test]
async fn revoking_one_subscribed_functions_invoke_permission_stops_only_its_deliveries() {
    let router = build_router();

    // The execution role the two functions are created with, and the sink their handlers
    // forward to. The sink is created with a visibility window past the whole suite, so a body
    // one round read cannot come back visible and be counted again in a later one.
    execution_role(&router).await;
    let created_sink = sqs(
        &router,
        "CreateQueue",
        &json!({
            "QueueName": SINK_QUEUE,
            "Attributes": { "VisibilityTimeout": "300" },
        }),
    )
    .await;
    let sink = created_sink["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateQueue must answer a QueueUrl, got {created_sink}"))
        .to_string();

    cloudformation(
        &router,
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", &template())],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, STACK, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "the stack holding the two subscribed functions must deploy, got {status} with {described}"
    );
    let outputs = outputs_of(&described);
    let alpha_arn = outputs
        .get("AlphaArn")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must publish AlphaArn, got {described}"));
    let beta_arn = outputs
        .get("BetaArn")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must publish BetaArn, got {described}"));

    for function in [ALPHA_FUNCTION, BETA_FUNCTION] {
        let state = wait_until_active(&router, function).await;
        assert_eq!(
            state, "Active",
            "setup: expected Lambda to report {function} Active, got {state:?}"
        );
    }

    // The topic and the two subscriptions, made through the API a client makes them through.
    let created = sns(&router, "CreateTopic", &[("Name", STACK)]).await;
    let topic = element(&created, "TopicArn");
    assert!(
        !topic.is_empty(),
        "setup: CreateTopic must answer a TopicArn, got {created}"
    );
    for endpoint in [&alpha_arn, &beta_arn] {
        let subscribed = sns(
            &router,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "lambda"),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
        let arn = element(&subscribed, "SubscriptionArn");
        assert!(
            arn.starts_with(&topic),
            "setup: subscribing {endpoint} must answer a confirmed subscription of the topic, \
             got {subscribed}"
        );
    }

    let mut seen: Vec<String> = Vec::new();
    let mut alpha = REVOKED;
    let mut beta = REVOKED;
    let mut expected_overall: Vec<(String, String)> = Vec::new();

    for (index, round) in ROUNDS.iter().enumerate() {
        let case = round.case;
        alpha = reconcile(&router, case, ALPHA_FUNCTION, &topic, alpha, round.alpha).await;
        beta = reconcile(&router, case, BETA_FUNCTION, &topic, beta, round.beta).await;

        // What each function's policy says is what the next publish is decided by, so the
        // document is read back before the publish rather than assumed from the calls above.
        for (function, grant) in [(ALPHA_FUNCTION, alpha), (BETA_FUNCTION, beta)] {
            let statements = policy_statements(&router, case, function).await;
            let expected = grant.granted.then(|| vec![STATEMENT_ID.to_string()]);
            assert_eq!(
                statements, expected,
                "case {case:?}: expected {function} to hold {expected:?} after the grant was \
                 reconciled, got {statements:?}"
            );
        }

        let marker = format!("marker-{index}");
        let published = sns(
            &router,
            "Publish",
            &[("TopicArn", &topic), ("Message", &marker)],
        )
        .await;
        assert!(
            !element(&published, "MessageId").is_empty(),
            "case {case:?}: Publish must answer a MessageId whether or not a delivery is \
             admitted, got {published}"
        );

        let mut wanted: Vec<String> = Vec::new();
        if round.alpha.granted {
            wanted.push(ALPHA_LABEL.to_string());
        }
        if round.beta.granted {
            wanted.push(BETA_LABEL.to_string());
        }
        wanted.sort();
        for label in &wanted {
            expected_overall.push((label.clone(), marker.clone()));
        }

        let forwarded = forwards_of(&router, &sink, &mut seen, &marker, wanted.len()).await;
        assert_eq!(
            forwarded, wanted,
            "case {case:?}: expected the functions {wanted:?} to forward {marker:?} and no \
             others to be invoked, got {forwarded:?} (every body seen so far: {seen:?})"
        );
    }

    // One last look, past the span a delivery takes: nothing arrived for a revoked grant after
    // the round that would have caught it, and nothing arrived twice.
    tokio::time::sleep(QUIET_BUDGET).await;
    seen.extend(drain(&router, &sink).await);
    let mut overall: Vec<(String, String)> = seen
        .iter()
        .filter_map(|body| body.split_once(LABEL_SEPARATOR))
        .map(|(label, message)| (label.to_string(), message.to_string()))
        .collect();
    overall.sort();
    expected_overall.sort();
    assert_eq!(
        overall, expected_overall,
        "expected exactly one forward per admitted delivery across every round, got {overall:?}"
    );
}
