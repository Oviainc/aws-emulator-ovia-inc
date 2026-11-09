// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed EventBridge rule INVOKES its Lambda function target, and the
//! sibling target no document admits is DEAD-LETTERED — end to end over the real
//! `MultiServiceRouter` with CloudFormation, EventBridge, Lambda, Amazon SQS and IAM mounted
//! on one engine.
//!
//! The rule pinned here: **a `PutEvents` entry is matched against every rule on the bus, and
//! each target of every matching ENABLED rule is handed the event; for a Lambda function
//! target "EventBridge relies on resource-based policies" (`PutTargets`), so the handover is
//! made as the service principal `events.amazonaws.com` and happens only where the FUNCTION's
//! own policy allows that principal `lambda:InvokeFunction` on it for the invoking rule — and
//! a handover that could not be made is written, once the target's `RetryPolicy` is
//! exhausted, to the queue the target's `DeadLetterConfig` names ("the target queue to send
//! dead-letter queue events to", `Target$DeadLetterConfig`), carrying the envelope that
//! target would have been handed, while a target the handover SUCCEEDS for dead-letters
//! nothing.** None of that depends on how the rule, the target or the grant came to exist: a
//! stack's `AWS::Events::Rule` and `AWS::Lambda::Permission` are the same rule, target and
//! statement a client's own `PutRule`/`PutTargets`/`AddPermission` leave behind.
//!
//! Five crates have to agree for that to hold — CloudFormation provisions the queues, their
//! policy, the functions, the rules with their per-target `DeadLetterConfig`/`RetryPolicy` and
//! the statement; EventBridge matches and hands over; Lambda decides the invocation against
//! the function's policy and then RUNS the handler; Amazon SQS holds both what the handler
//! wrote and what was dead-lettered; IAM holds the execution role the handler's own write is
//! judged against — so the suite lives here rather than in any one crate. Every observation is
//! made where a client makes it: on the three queues the stack publishes, never on an emulator
//! internal.
//!
//! The rows differ along exactly what the rule generalises over: WHICH rule the stack's
//! `AWS::Lambda::Permission` scopes the grant to, whether it scopes it at all, and whether the
//! stack declares the statement. Each row deploys the same two-rule shape — two rules with the
//! SAME event pattern, each with one function target and its own dead-letter queue, and a
//! grant for at most one of the two functions — so the permitted and the un-permitted target
//! are asserted against each other in every row, and the un-permitted sibling's dead letter is
//! what makes the boundary observable. Each row also publishes an event the pattern does not
//! name, which must reach none of the three queues.
//!
//! `crate::events_lambda_target_resource_policy` is the suite for which policy shapes admit
//! the invocation at all, and `crate::events_lambda_target_dead_letter_capture` for what the
//! dead-lettered message carries; this one is the suite for the whole deployed pipeline, from
//! the template to the handler's own work product.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::outputs_of;
use crate::sns_lambda_subscription_permission_boundary::{
    cloudformation, iam, stayed_quiet, trust_policy, wait_for, ACCOUNT, CREATE_COMPLETE,
    LOOK_INTERVAL,
};

/// The Region every request here is signed for — the one `wait_for_stack_status` polls in.
const REGION: &str = "us-east-1";

/// The `source` and `detail-type` both deployed rules match on, and a `source` neither names.
const SOURCE: &str = "sim2real.events.boundary";
const DETAIL_TYPE: &str = "Target Permission Probe";
const OTHER_SOURCE: &str = "sim2real.events.unmatched";

/// The service principal EventBridge hands a resource-policy-governed target over as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The IAM action an invocation of a function performs, which is what a grant for this
/// service has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The execution role the declared functions are created with, provisioned in IAM before the
/// stack so the template names a role that service really holds.
const ROLE_NAME: &str = "events-target-boundary-exec";

/// How many times a function's state is polled before the suite gives up on it.
const POLLS: usize = 400;

/// CloudFormation, EventBridge, Lambda, Amazon SQS and IAM over ONE engine, as
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
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
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
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` GET, as its parsed body.
async fn lambda_get(router: &MultiServiceRouter, uri: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// Poll `GetFunctionConfiguration` until Lambda first reports `name` `Active` — "the function
/// is ready to be invoked" (`FunctionConfiguration$State`). The span is that service's own, so
/// nothing here sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    for _ in 0..POLLS {
        let reported = lambda_get(
            router,
            &format!("/2015-03-31/functions/{name}/configuration"),
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: Lambda never reported {name} Active");
}

/// The identity policy the execution role carries, so the write the PERMITTED function's
/// handler makes is served: a handler calls another service with temporary credentials for
/// its execution role, so resolving the success queue's URL by name and the write itself are
/// both judged against the policies in force on that role.
fn forwarding_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["sqs:GetQueueUrl", "sqs:SendMessage"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Create the execution role the template names, where IAM holds it: trusted by Lambda, and
/// granting the write its handler makes.
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
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "write-the-success-queue"),
            ("PolicyDocument", &forwarding_policy()),
        ],
    )
    .await;
}

/// The handler the PERMITTED function's `AWS::Lambda::Function` declares inline: it writes the
/// envelope it was handed to the success queue the same stack declares, which is where the
/// invocation is observed. The queue is addressed by NAME through the service's own
/// `GetQueueUrl`, so the source carries nothing the deployment had to compute.
const FORWARDING_SOURCE: &str = r#"
import json
import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName="SUCCESS_QUEUE_NAME")["QueueUrl"]
    client.send_message(QueueUrl=url, MessageBody=json.dumps(event))
    return {"forwarded": 1}
"#;

/// The handler the UN-PERMITTED function declares. Nothing ever runs it — the row's assertion
/// is that it does not — so it computes nothing.
const IDLE_SOURCE: &str = r#"
def lambda_handler(event, context):
    return {"ran": True}
"#;

/// What the stack's `AWS::Lambda::Permission` for the PERMITTED function declares, which is
/// the one thing the rows differ in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No `AWS::Lambda::Permission` resource at all, so the function carries no policy.
    Absent,
    /// The grant declared with no `SourceArn`, so it is not scoped to a rule.
    Unscoped,
    /// The grant scoped by `SourceArn` to the rule that targets this function.
    ItsOwnRule,
    /// The grant scoped by `SourceArn` to the SIBLING rule, which targets the other function.
    TheSiblingRule,
}

/// One row: what the stack grants on the permitted-target function, and whether EventBridge
/// may therefore hand that target the event.
struct Row {
    case: &'static str,
    stack: &'static str,
    grant: Grant,
    /// Whether the first function is invoked, so its handler's work product reaches the
    /// success queue and its own dead-letter queue stays empty.
    invoked: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the permission is scoped by SourceArn to the rule that targets the function",
            stack: "events-target-boundary-own",
            grant: Grant::ItsOwnRule,
            invoked: true,
        },
        Row {
            case: "the permission grants this service the invocation unscoped",
            stack: "events-target-boundary-unscoped",
            grant: Grant::Unscoped,
            invoked: true,
        },
        Row {
            case: "the permission is scoped by SourceArn to the sibling rule",
            stack: "events-target-boundary-sibling",
            grant: Grant::TheSiblingRule,
            invoked: false,
        },
        Row {
            case: "the stack declares no AWS::Lambda::Permission at all",
            stack: "events-target-boundary-none",
            grant: Grant::Absent,
            invoked: false,
        },
    ]
}

/// The template one row deploys: a success queue, a dead-letter queue per target, the queue
/// policy that admits this service's dead-letter writes, two functions, the row's grant, and
/// two rules carrying the SAME event pattern, each with one function target, that target's own
/// `DeadLetterConfig` and `MaximumRetryAttempts` 0 so an undeliverable handover is dead-lettered
/// at once.
fn template(stack: &str, grant: Grant) -> String {
    let success = format!("{stack}-success");
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
    });
    let mut resources = json!({
        "Success": {
            "Type": "AWS::SQS::Queue",
            "Properties": { "QueueName": success },
        },
        "DlqGood": {
            "Type": "AWS::SQS::Queue",
            "Properties": { "QueueName": format!("{stack}-dlq-good") },
        },
        "DlqBad": {
            "Type": "AWS::SQS::Queue",
            "Properties": { "QueueName": format!("{stack}-dlq-bad") },
        },
        "DlqPolicy": {
            "Type": "AWS::SQS::QueuePolicy",
            "Properties": {
                "Queues": [{ "Ref": "DlqGood" }, { "Ref": "DlqBad" }],
                "PolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": EVENTS_PRINCIPAL },
                        "Action": "sqs:SendMessage",
                        "Resource": [
                            { "Fn::GetAtt": ["DlqGood", "Arn"] },
                            { "Fn::GetAtt": ["DlqBad", "Arn"] },
                        ],
                    }],
                },
            },
        },
        "Good": {
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": format!("{stack}-good"),
                "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                "Runtime": "python3.12",
                "Handler": "index.lambda_handler",
                "Timeout": 10,
                "Code": { "ZipFile": FORWARDING_SOURCE.replace("SUCCESS_QUEUE_NAME", &success) },
            },
        },
        "Bad": {
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": format!("{stack}-bad"),
                "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                "Runtime": "python3.12",
                "Handler": "index.lambda_handler",
                "Timeout": 10,
                "Code": { "ZipFile": IDLE_SOURCE },
            },
        },
        "GoodRule": {
            "Type": "AWS::Events::Rule",
            "Properties": {
                "Name": format!("{stack}-good-rule"),
                "EventPattern": pattern,
                "State": "ENABLED",
                "Targets": [{
                    "Id": "good",
                    "Arn": { "Fn::GetAtt": ["Good", "Arn"] },
                    "RetryPolicy": { "MaximumRetryAttempts": 0 },
                    "DeadLetterConfig": { "Arn": { "Fn::GetAtt": ["DlqGood", "Arn"] } },
                }],
            },
        },
        "BadRule": {
            "Type": "AWS::Events::Rule",
            "Properties": {
                "Name": format!("{stack}-bad-rule"),
                "EventPattern": pattern,
                "State": "ENABLED",
                "Targets": [{
                    "Id": "bad",
                    "Arn": { "Fn::GetAtt": ["Bad", "Arn"] },
                    "RetryPolicy": { "MaximumRetryAttempts": 0 },
                    "DeadLetterConfig": { "Arn": { "Fn::GetAtt": ["DlqBad", "Arn"] } },
                }],
            },
        },
    });
    let scope = match grant {
        Grant::Absent => None,
        Grant::Unscoped => Some(Value::Null),
        Grant::ItsOwnRule => Some(json!({ "Fn::GetAtt": ["GoodRule", "Arn"] })),
        Grant::TheSiblingRule => Some(json!({ "Fn::GetAtt": ["BadRule", "Arn"] })),
    };
    if let Some(scope) = scope {
        let mut properties = json!({
            "FunctionName": { "Fn::GetAtt": ["Good", "Arn"] },
            "Action": INVOKE_ACTION,
            "Principal": EVENTS_PRINCIPAL,
        });
        if !scope.is_null() {
            properties["SourceArn"] = scope;
        }
        resources["InvokeGood"] = json!({
            "Type": "AWS::Lambda::Permission",
            "Properties": properties,
        });
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": {
            "SuccessUrl": { "Value": { "Ref": "Success" } },
            "DlqGoodUrl": { "Value": { "Ref": "DlqGood" } },
            "DlqBadUrl": { "Value": { "Ref": "DlqBad" } },
        },
    })
    .to_string()
}

/// Publish one entry and assert EventBridge accepted it, whatever the deliveries then come
/// to: "FailedEntryCount — the number of failed entries", which a matched-but-undeliverable
/// target is not one of.
async fn publish(router: &MultiServiceRouter, case: &str, source: &str, marker: &str) {
    let answered = events(
        router,
        case,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": source,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "backupJobId": marker, "state": "COMPLETED" }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        answered["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the entry to be accepted, got {answered}"
    );
    let entries = answered["Entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected one answered entry, got {answered}"
    );
    assert!(
        entries[0]["EventId"].as_str().is_some_and(|id| !id.is_empty()),
        "case {case:?}: expected the accepted entry to carry an EventId, got {answered}"
    );
}

/// Every row: the stack's grant decides which of the two same-pattern rules really invokes its
/// function target, the un-permitted sibling's event is dead-lettered to the queue its
/// `DeadLetterConfig` names, and an event the pattern does not name reaches nowhere.
#[tokio::test]
async fn a_deployed_rules_permitted_lambda_target_runs_and_its_unpermitted_sibling_dead_letters()
{
    for row in rows() {
        let case = row.case;
        let router = build_router();
        execution_role(&router).await;

        cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row.stack, row.grant)),
            ],
        )
        .await;
        let (status, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to deploy, got {status} with {described}"
        );
        let outputs = outputs_of(&described);
        let url = |name: &str| {
            outputs.get(name).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: expected the stack to publish {name}, got {described}")
            })
        };
        let success_url = url("SuccessUrl");
        let good_dlq_url = url("DlqGoodUrl");
        let bad_dlq_url = url("DlqBadUrl");

        // Both functions have to be invocable before the event is published: "During this
        // time, you can't invoke or modify the function" (`CreateFunction`), so a handover
        // withheld here would be the lifecycle's refusal and not the policy's.
        wait_until_active(&router, case, &format!("{}-good", row.stack)).await;
        wait_until_active(&router, case, &format!("{}-bad", row.stack)).await;

        // An entry neither rule's pattern names, published first so the waits below give it
        // every chance to arrive somewhere it must not.
        let unmatched = format!("{}-unmatched", row.stack);
        publish(&router, case, OTHER_SOURCE, &unmatched).await;

        let marker = format!("{}-matched", row.stack);
        publish(&router, case, SOURCE, &marker).await;

        // The un-permitted sibling is the control that is the same in every row: its function
        // carries no statement at all, so its handover cannot be made and the envelope —
        // still carrying the published detail — lands on the queue its target names.
        let dead_lettered = wait_for(&router, &bad_dlq_url, &marker).await;
        assert!(
            dead_lettered.iter().any(|body| body.contains(&marker)),
            "case {case:?}: expected the un-permitted target's event to be dead-lettered to \
             its DeadLetterConfig queue carrying {marker:?}, got {dead_lettered:?}"
        );

        let (on_success, on_good_dlq) = if row.invoked {
            let forwarded = wait_for(&router, &success_url, &marker).await;
            assert!(
                forwarded.iter().any(|body| body.contains(&marker)),
                "case {case:?}: expected the permitted target's handler to run and write the \
                 envelope carrying {marker:?} to the success queue, got {forwarded:?}"
            );
            let quiet = stayed_quiet(&router, &good_dlq_url).await;
            assert!(
                quiet.is_empty(),
                "case {case:?}: expected the permitted target to dead-letter nothing, got \
                 {quiet:?}"
            );
            (forwarded, quiet)
        } else {
            let quiet = stayed_quiet(&router, &success_url).await;
            assert!(
                quiet.is_empty(),
                "case {case:?}: expected the un-granted function never to run, so nothing \
                 reaches the success queue, got {quiet:?}"
            );
            let withheld = wait_for(&router, &good_dlq_url, &marker).await;
            assert!(
                withheld.iter().any(|body| body.contains(&marker)),
                "case {case:?}: expected the target no statement admits to be dead-lettered \
                 to its own DeadLetterConfig queue carrying {marker:?}, got {withheld:?}"
            );
            (quiet, withheld)
        };

        // Nothing reached any of the three queues for the entry neither pattern names. Every
        // body above was read AFTER that entry was published and while the matched one was
        // waited for, so a delivery of it would stand among them.
        for (queue, held) in [
            ("the success queue", &on_success),
            ("the permitted target's dead-letter queue", &on_good_dlq),
            ("the un-permitted target's dead-letter queue", &dead_lettered),
        ] {
            assert!(
                !held.iter().any(|body| body.contains(&unmatched)),
                "case {case:?}: expected the event neither pattern names to reach nothing, but \
                 {queue} holds {held:?}"
            );
        }
    }
}
