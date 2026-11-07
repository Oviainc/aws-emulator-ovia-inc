// SPDX-License-Identifier: CC0-1.0

//! The ERROR NAME and the CAUSE a Step Functions `Task` state reports when the service
//! its integration called REFUSED the call — driven across Step Functions, IAM, Amazon
//! SQS and Lambda, mounted on ONE engine exactly as `core/router/src/lib.rs` mounts
//! them.
//!
//! RULE: a failed service-integration `Task` is reported under the exception the invoked
//! service's own SDK client raises for the refusal — the service as Step Functions names
//! it, then the class of the error SHAPE the answer named, which is that shape's name
//! with the `Exception` suffix the SDKs give it — and under that service's BASE
//! exception where the answer named no shape of its model: one of AWS's common errors,
//! or a shape of the request framework in front of the service. The cause is that
//! exception's own message: the service's sentence, then the metadata the answer carried
//! — the client it was made with, the status, the error code (the LEGACY query code for
//! a query-compatible service, which is the code its own client matches on) and the
//! identifier of the request.
//!
//! Two crates have to agree for that to hold — Amazon SQS and Lambda answer the refusal,
//! Step Functions names it — so the suite lives here rather than in either of them, and
//! borrows the four-service wiring `sfn_task_target_authorization` already owns.
//!
//! The rows differ along what the rule generalises over: whether the shape the answer
//! named belongs to the invoked service's model or to the framework in front of it,
//! whether its name already carries the suffix, and which service answered.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sfn_task_target_authorization::{
    allow, emulator, logs_only, machine, queue, queue_handles, ran, role, sqs, string,
    task_failure, ACCOUNT, REGION,
};

/// A FIFO queue called `name`; answers its url and its ARN. "You can provide this
/// attribute only during queue creation", so the type is the create's own.
async fn fifo_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(
        router,
        "CreateQueue",
        &json!({ "QueueName": name, "Attributes": { "FifoQueue": "true" } }),
    )
    .await;
    queue_handles(router, &created).await
}

/// One row: the case, the `Resource` the `Task` names, the parameters it calls with, the
/// error name the task must report, and the fragments its cause must carry.
struct Row {
    case: &'static str,
    resource: &'static str,
    parameters: Value,
    error: &'static str,
    carries: Vec<String>,
}

/// A refused integration call is reported under the invoked service's own SDK exception,
/// with that exception's own message as the cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_integration_call_is_named_after_the_services_own_exception() {
    let router = emulator();
    let (standard_url, standard_arn) = queue(&router, "refusal-standard").await;
    let (fifo_url, fifo_arn) = fifo_queue(&router, "refusal-ordered.fifo").await;
    // A queue url no queue answers to, addressed exactly as Amazon SQS addresses one.
    let absent_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT}/refusal-absent");
    let absent_queue_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:refusal-absent");
    let absent_function = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:refusal-absent");
    // Every call below is one the role MAY make: the refusal under test is the invoked
    // service's own answer, so nothing may be stopped by the authorization gate.
    let role_arn = role(
        &router,
        "RefusalNamesRole",
        &json!({
            "Version": "2012-10-17",
            "Statement": [
                logs_only(),
                allow("sqs:SendMessage", &standard_arn),
                allow("sqs:SendMessage", &fifo_arn),
                allow("sqs:SendMessage", &absent_queue_arn),
                allow("lambda:InvokeFunction", &absent_function),
            ],
        }),
    )
    .await;

    let rows = [
        Row {
            // "If you do not provide a MessageGroupId when sending a message to a FIFO
            // queue, the action fails": the parameter the request had to carry is
            // refused by the framework in front of the service, which names no shape of
            // its model — so the base exception carries it, and the legacy code the
            // answer also names is what the exception reports.
            case: "a parameter a FIFO send had to carry",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": fifo_url.clone(), "MessageBody": "ordered" }),
            error: "SQS.AmazonSQSException",
            carries: vec![
                "MessageGroupId".to_string(),
                "(Service: AmazonSQS; Status Code: 400; Error Code: MissingParameter;".to_string(),
            ],
        },
        Row {
            // `QueueDoesNotExist` is a shape Amazon SQS's own model declares, spelled
            // without the suffix its SDK class carries.
            case: "a queue the service does not hold",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": absent_url.clone(), "MessageBody": "nowhere" }),
            error: "SQS.QueueDoesNotExistException",
            carries: vec![
                "The specified queue does not exist.".to_string(),
                "(Service: AmazonSQS; Status Code: 400; Error Code: \
                 AWS.SimpleQueueService.NonExistentQueue;"
                    .to_string(),
            ],
        },
        Row {
            // "When you set FifoQueue, you can't set DelaySeconds per message": a value
            // the queue's type does not take is one of AWS's common errors, so the base
            // exception carries this one too.
            case: "a value the queue's type does not take",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({
                "QueueUrl": fifo_url.clone(),
                "MessageBody": "delayed",
                "MessageGroupId": "group",
                "DelaySeconds": 5,
            }),
            error: "SQS.AmazonSQSException",
            carries: vec![
                "(Service: AmazonSQS; Status Code: 400; Error Code: InvalidParameterValue;"
                    .to_string(),
            ],
        },
        Row {
            // The same rule through the OTHER family, which makes the same call on the
            // same queue.
            case: "the same refusal through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:sqs:sendMessage",
            parameters: json!({ "QueueUrl": absent_url.clone(), "MessageBody": "nowhere" }),
            error: "SQS.QueueDoesNotExistException",
            carries: vec!["The specified queue does not exist.".to_string()],
        },
        Row {
            // A second service, and the arm of the rule where the shape's own name
            // already carries the suffix: it keeps the one it has.
            case: "a function Lambda does not hold",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": absent_function.clone(), "Payload": {} }),
            error: "Lambda.ResourceNotFoundException",
            carries: vec![
                "(Service: AWSLambda; Status Code: 404; Error Code: ResourceNotFoundException;"
                    .to_string(),
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_arn = machine(
            &router,
            &format!("refusal-{index}"),
            row.resource,
            &row.parameters,
            &role_arn,
        )
        .await;
        let (execution_arn, described) = ran(&router, &machine_arn, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(row.error),
            "case {case:?}: expected the error {:?}, got {described:?}",
            row.error
        );
        let details = task_failure(&router, &execution_arn, case).await;
        let cause = string(&details, "cause");
        for fragment in &row.carries {
            assert!(
                cause.contains(fragment.as_str()),
                "case {case:?}: expected the cause to carry {fragment:?}, got {cause:?}"
            );
        }
        // Every one of them is the rendering of an SDK exception, which ends in the
        // metadata the answer carried.
        assert!(
            cause.ends_with("Proxy: null)"),
            "case {case:?}: expected the SDK exception's own message, got {cause:?}"
        );
        // The cause is the same sentence the execution's own record reports.
        assert_eq!(
            described.get("cause").and_then(Value::as_str),
            Some(cause.as_str()),
            "case {case:?}: expected the execution to report the task's own cause"
        );
    }

    // Nothing was sent by any of the refused calls: the standard queue was never
    // addressed, and the FIFO one refused both of its sends.
    for (name, url) in [("standard", &standard_url), ("fifo", &fifo_url)] {
        let received = sqs(
            &router,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert!(
            received.get("Messages").is_none(),
            "the {name} queue must hold nothing a refused send left, got {received:?}"
        );
    }
}
