// SPDX-License-Identifier: CC0-1.0

//! Whether an EventBridge rule INVOKES its Lambda function target is decided by that
//! function's own resource-based policy, end to end over the wire through the real
//! `MultiServiceRouter` with EventBridge, Lambda and SQS mounted on one engine.
//!
//! The rule pinned here: "for Lambda functions, Amazon SNS topics and Amazon SQS queues,
//! EventBridge relies on resource-based policies" (`PutTargets`), so a delivery to such a
//! target is made as the service principal `events.amazonaws.com` and the TARGET's own
//! document is the only one in force over it — a function whose policy does not allow that
//! principal `lambda:InvokeFunction` on it for the invoking rule is never invoked, and the
//! event goes to the target's dead-letter queue carrying the permission condition instead.
//! A function that carries no policy at all, and a target that is no longer there, are both
//! in that position: there is no document admitting the invocation either way.
//!
//! The rows differ along what the rule generalises over: whether the function carries a
//! policy at all, WHICH principal the grant names, WHICH action it names, whether it is
//! scoped by `aws:SourceArn` and whether that scope names the invoking rule, and whether
//! the target resource exists. Each row asserts all three observables of the decision — what
//! EventBridge recorded for the handover, what the function's own asynchronous queue holds,
//! and what reached the dead-letter queue — so a refusal that still invoked, or an
//! invocation that still dead-lettered, fails here.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{InvocationOutcome, NO_PERMISSIONS};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = "000000000000";

/// The `source` and `detail-type` every event this suite publishes carries, and what the
/// rules match on.
const SOURCE: &str = "lambda.target.policy";
const DETAIL_TYPE: &str = "Policy Probe";

/// A deployment package as `Code.ZipFile` carries it. Its content does not matter here:
/// only that Lambda accepts the function and records the invocation EventBridge makes.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// Past the span a created function spends in `Pending` before it is `Active`, so a
/// delivery that arrives after it is one the function's lifecycle does not refuse.
const PAST_THE_FUNCTION_TRANSITION: Duration = ACTIVATION_DELAY.saturating_add(
    Duration::from_millis(300),
);

/// The IAM action an invocation of a function performs, which is what a grant for this
/// service has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The principal EventBridge invokes a resource-policy-governed target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The role a created function names. No IAM store is mounted here, so it is a value the
/// create carries rather than a record this suite relies on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/lambda-exec")
}

/// EventBridge, Lambda and SQS over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them.
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

/// A dead-letter queue this service may send to, so what reaches it is decided by the
/// FUNCTION's policy rather than by the queue's: the queue's own document admits the send
/// ("for … Amazon SQS queues, EventBridge relies on resource-based policies").
async fn create_dead_letter_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
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
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
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

/// Create a function and answer the ARN Lambda assigned it.
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
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// Add one statement to a function's resource-based policy through `AddPermission`, which
/// is how a client grants this service the invocation.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    statement_id: &str,
    principal: &str,
    action: &str,
    source_arn: Option<&str>,
) {
    let mut body = json!({
        "StatementId": statement_id,
        "Action": action,
        "Principal": principal,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    let (status, answer) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        body,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} ({answer})"
    );
}

/// What EventBridge recorded for each handover of a routed event: the target ARN and the
/// outcome.
fn handovers(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, InvocationOutcome)> {
    let state = engine.state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    let store = state.lock();
    store
        .invocations
        .iter()
        .map(|invocation| (invocation.target_arn.clone(), invocation.outcome.clone()))
        .collect()
}

/// How many asynchronous invocations Lambda holds for `function` — what an accepted `Event`
/// invocation, the one EventBridge makes for a function target, persists.
fn queued_invocations(engine: &Arc<engine::InMemoryEngine>, function: &str) -> usize {
    let state = engine
        .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map_or(0, |record| record.async_invocations.len())
}

/// Every message waiting on the queue at `url`, with its attributes.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answer = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    answer["Messages"].as_array().cloned().unwrap_or_default()
}

/// What one row grants on the function before the event is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No `AddPermission` at all, so the function carries no policy.
    None,
    /// The service principal allowed the invocation, unscoped.
    Invoke,
    /// The same grant, scoped by `aws:SourceArn` to the rule that invokes it.
    InvokeFromThisRule,
    /// The same grant, scoped by `aws:SourceArn` to another rule.
    InvokeFromAnotherRule,
    /// The invocation allowed to ANOTHER service principal.
    InvokeByAnotherPrincipal,
    /// Another action allowed to this service principal.
    AnotherActionForThisPrincipal,
}

/// One row: what the function's policy says, and whether the delivery happens.
struct Row {
    case: &'static str,
    grant: Grant,
    /// Whether the function is deleted again after the target is written, so the delivery
    /// finds no function and no policy.
    delete_function: bool,
    /// Whether EventBridge invokes the function.
    invoked: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the function carries no resource-based policy at all",
            grant: Grant::None,
            delete_function: false,
            invoked: false,
        },
        Row {
            case: "the policy allows this service the invocation, unscoped",
            grant: Grant::Invoke,
            delete_function: false,
            invoked: true,
        },
        Row {
            case: "the grant is scoped by SourceArn to the invoking rule",
            grant: Grant::InvokeFromThisRule,
            delete_function: false,
            invoked: true,
        },
        Row {
            case: "the grant is scoped by SourceArn to another rule",
            grant: Grant::InvokeFromAnotherRule,
            delete_function: false,
            invoked: false,
        },
        Row {
            case: "the invocation is granted to another service principal",
            grant: Grant::InvokeByAnotherPrincipal,
            delete_function: false,
            invoked: false,
        },
        Row {
            case: "the grant names another action for this service principal",
            grant: Grant::AnotherActionForThisPrincipal,
            delete_function: false,
            invoked: false,
        },
        Row {
            case: "the target function was deleted after the target was written",
            grant: Grant::Invoke,
            delete_function: true,
            invoked: false,
        },
    ]
}

/// The whole decision, per row: the handover's outcome, the function's queue and the
/// dead-letter queue.
#[tokio::test]
async fn a_function_targets_own_resource_policy_decides_the_delivery() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = router();
        let function_name = format!("policy-probe-{index}");
        let rule_name = format!("policy-probe-rule-{index}");
        let (dead_letter_url, dead_letter_arn) =
            create_dead_letter_queue(&router, case, &format!("policy-probe-dlq-{index}")).await;
        let function_arn = create_function(&router, case, &function_name).await;

        let rule = events(
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
        let rule_arn = rule["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
            .to_string();
        let another_rule_arn = format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/some-other-rule");

        match row.grant {
            Grant::None => {}
            Grant::Invoke => {
                add_permission(
                    &router, case, &function_name, "events-invoke", EVENTS_PRINCIPAL,
                    INVOKE_ACTION, None,
                )
                .await;
            }
            Grant::InvokeFromThisRule => {
                add_permission(
                    &router,
                    case,
                    &function_name,
                    "events-invoke-this-rule",
                    EVENTS_PRINCIPAL,
                    INVOKE_ACTION,
                    Some(&rule_arn),
                )
                .await;
            }
            Grant::InvokeFromAnotherRule => {
                add_permission(
                    &router,
                    case,
                    &function_name,
                    "events-invoke-other-rule",
                    EVENTS_PRINCIPAL,
                    INVOKE_ACTION,
                    Some(&another_rule_arn),
                )
                .await;
            }
            Grant::InvokeByAnotherPrincipal => {
                add_permission(
                    &router,
                    case,
                    &function_name,
                    "sns-invoke",
                    "sns.amazonaws.com",
                    INVOKE_ACTION,
                    None,
                )
                .await;
            }
            Grant::AnotherActionForThisPrincipal => {
                add_permission(
                    &router,
                    case,
                    &function_name,
                    "events-get",
                    EVENTS_PRINCIPAL,
                    "lambda:GetFunction",
                    None,
                )
                .await;
            }
        }

        events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [{
                    "Id": "fn",
                    "Arn": function_arn,
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                }],
            }),
        )
        .await;

        if row.delete_function {
            let (status, answer) = lambda(
                &router,
                "DELETE",
                &format!("/2015-03-31/functions/{function_name}"),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected HTTP 204 from Lambda DeleteFunction, got {status} \
                 ({answer})"
            );
        }

        // The created function leaves `Pending` by the clock, so the delivery is made past
        // that span: a refusal here has to be the policy's and not the lifecycle's.
        tokio::time::sleep(PAST_THE_FUNCTION_TRANSITION).await;

        events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({ "probe": index }).to_string(),
                }],
            }),
        )
        .await;

        let recorded = handovers(&engine);
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: expected one recorded handover, got {recorded:?}"
        );
        let (target, outcome) = &recorded[0];
        assert_eq!(
            target, &function_arn,
            "case {case:?}: expected the handover to name {function_arn}, got {target:?}"
        );

        let queued = queued_invocations(&engine, &function_name);
        let dead_letters = drain(&router, case, &dead_letter_url).await;

        if row.invoked {
            assert_eq!(
                outcome,
                &InvocationOutcome::Invoked,
                "case {case:?}: expected the delivery to be invoked, got {outcome:?}"
            );
            assert_eq!(
                queued, 1,
                "case {case:?}: expected the function to hold one queued invocation, got {queued}"
            );
            assert!(
                dead_letters.is_empty(),
                "case {case:?}: expected no dead letter, got {dead_letters:?}"
            );
            continue;
        }

        let InvocationOutcome::Failed { code, message } = outcome else {
            panic!("case {case:?}: expected a failed delivery, got {outcome:?}");
        };
        assert_eq!(
            code, NO_PERMISSIONS,
            "case {case:?}: expected the condition {NO_PERMISSIONS}, got {code:?}"
        );
        // The sentence is the one AWS Lambda answers a caller its resource policy does not
        // permit: the principal the call was made as, the action and the function.
        let expected_opening = format!(
            "User: {EVENTS_PRINCIPAL} is not authorized to perform: {INVOKE_ACTION} on \
             resource: {function_arn} because no resource-based policy allows the \
             {INVOKE_ACTION} action"
        );
        assert!(
            message.starts_with(&expected_opening),
            "case {case:?}: expected the refusal to open {expected_opening:?}, got {message:?}"
        );
        assert_eq!(
            queued, 0,
            "case {case:?}: expected the function never to be invoked, got {queued} queued \
             invocation(s)"
        );
        assert_eq!(
            dead_letters.len(),
            1,
            "case {case:?}: expected one dead letter, got {dead_letters:?}"
        );
        let attributes = &dead_letters[0]["MessageAttributes"];
        assert_eq!(
            attributes["ERROR_CODE"]["StringValue"],
            json!(NO_PERMISSIONS),
            "case {case:?}: expected the dead-letter ERROR_CODE {NO_PERMISSIONS}, got {:?}",
            dead_letters[0]
        );
        let reported = attributes["ERROR_MESSAGE"]["StringValue"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reported, message,
            "case {case:?}: the dead-letter ERROR_MESSAGE is the recorded refusal, got \
             {reported:?}"
        );
    }
}
