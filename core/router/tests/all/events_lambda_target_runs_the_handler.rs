// SPDX-License-Identifier: CC0-1.0

//! An EventBridge rule's Lambda target invocation is ASYNCHRONOUS, and the event it queues
//! is then DELIVERED — end to end over the real `MultiServiceRouter` with EventBridge,
//! Lambda and Amazon SQS mounted on one engine.
//!
//! The rule pinned here: EventBridge invokes a function target asynchronously ("Event –
//! Invoke the function asynchronously", `Invoke$InvocationType`), so the handover leaves a
//! queued invocation on the function, and that entry does not stay queued: Lambda itself
//! attempts the delivery once the entry's attempt is due, with no caller asking for it, and
//! the entry records what the attempt came to. An entry still `Queued` long after its
//! attempt was due is a function the rule never ran.
//!
//! What the delivered run then DOES — the handler executing its deployment package and
//! writing through the AWS SDK its runtime supplies — is asserted where a function's own
//! package is built, in that service crate's
//! `tests/all/async_delivery.rs` and `tests/all/handler_aws_sdk.rs`. This suite is the
//! cross-service half: that the rule's handover reaches Lambda's asynchronous queue and
//! that the queue is drained by Lambda itself.
//!
//! The rows differ along what the rule generalises over: how many matching events are
//! published, and whether the published event matches the rule's pattern at all.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ComAmazonawsLambdaAwsgirapiserviceState, ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries, and what the
/// rule matches on.
const SOURCE: &str = "lambda.target.delivery";
const DETAIL_TYPE: &str = "Delivery Probe";

/// A `source` the rule's pattern does not name.
const OTHER_SOURCE: &str = "lambda.target.other";

/// The principal EventBridge invokes a resource-policy-governed target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The IAM action an invocation of a function performs.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A deployment package as `Code.ZipFile` carries it. Its contents do not matter here: this
/// suite asserts that the queued delivery is ATTEMPTED, not what a handler computed.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// A well-formed execution role ARN.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(200);

/// How long it waits for work the clock owes before giving up on it.
const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// EventBridge, Lambda and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("authorization", authorization("events"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, Value) {
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

/// The outcome of every asynchronous invocation Lambda holds for `function`.
fn queued(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
) -> Vec<AsyncInvocationOutcome> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| {
            record
                .async_invocations
                .iter()
                .map(|entry| entry.outcome.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until `function` holds `expected` entries and none of them is still `Queued`.
async fn delivered(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    expected: usize,
) -> Vec<AsyncInvocationOutcome> {
    let looks = DELIVERY_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let held = queued(engine, function);
        if held.len() == expected
            && held.iter().all(|outcome| {
                !matches!(
                    outcome,
                    AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
                )
            })
        {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    queued(engine, function)
}

/// What EventBridge recorded for each handover of a routed event.
fn handovers(engine: &Arc<engine::InMemoryEngine>) -> Vec<InvocationOutcome> {
    let state = engine.state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    let store = state.lock();
    store
        .invocations
        .iter()
        .map(|invocation| invocation.outcome.clone())
        .collect()
}

/// One row: how many events are published, with which `source`, and how many deliveries
/// that must lead to.
struct Row {
    case: &'static str,
    source: &'static str,
    published: usize,
    deliveries: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one matching event queues and delivers one invocation",
            source: SOURCE,
            published: 1,
            deliveries: 1,
        },
        Row {
            case: "three matching events queue and deliver one invocation each",
            source: SOURCE,
            published: 3,
            deliveries: 3,
        },
        Row {
            case: "an event the rule's pattern does not match queues nothing",
            source: OTHER_SOURCE,
            published: 1,
            deliveries: 0,
        },
    ]
}

/// Every row: the rule's handover reaches Lambda's asynchronous queue, and Lambda drains
/// that queue itself.
#[tokio::test]
async fn a_rules_lambda_target_delivery_is_queued_and_then_attempted() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = router();
        let function_name = format!("delivery-probe-{index}");
        let (status, function) = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            json!({
                "FunctionName": function_name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({function})"
        );
        let function_arn = function["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {function}"))
            .to_string();

        // "For Lambda functions … EventBridge relies on resource-based policies", so the
        // grant that admits the invocation is added before the rule is wired.
        let (status, granted) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function_name}/policy"),
            json!({
                "StatementId": "events-invoke",
                "Action": INVOKE_ACTION,
                "Principal": EVENTS_PRINCIPAL,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from AddPermission, got {status} ({granted})"
        );

        let rule_name = format!("delivery-probe-rule-{index}");
        events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule_name,
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [DETAIL_TYPE],
                })
                .to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [{ "Id": "fn", "Arn": function_arn }],
            }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the target to be accepted, got {wired}"
        );

        // The created function leaves `Pending` by the clock, so the handover is made past
        // that span.
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

        for probe in 0..row.published {
            let published = events(
                &router,
                case,
                "PutEvents",
                &json!({
                    "Entries": [{
                        "Source": row.source,
                        "DetailType": DETAIL_TYPE,
                        "Detail": json!({ "probe": probe }).to_string(),
                    }],
                }),
            )
            .await;
            assert_eq!(
                published["FailedEntryCount"],
                json!(0),
                "case {case:?}: expected the event to be accepted, got {published}"
            );
        }

        let settled = delivered(&engine, &function_name, row.deliveries).await;
        assert_eq!(
            settled.len(),
            row.deliveries,
            "case {case:?}: expected {} queued invocation(s), got {settled:?}",
            row.deliveries
        );
        for outcome in &settled {
            assert!(
                !matches!(
                    outcome,
                    AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
                ),
                "case {case:?}: expected every queued delivery to be attempted and settled, \
                 got {settled:?}"
            );
        }
        let recorded = handovers(&engine);
        assert_eq!(
            recorded.len(),
            row.deliveries,
            "case {case:?}: expected {} recorded handover(s), got {recorded:?}",
            row.deliveries
        );
        for outcome in &recorded {
            assert_eq!(
                outcome,
                &InvocationOutcome::Invoked,
                "case {case:?}: expected the handover to be invoked, got {recorded:?}"
            );
        }
    }
}
