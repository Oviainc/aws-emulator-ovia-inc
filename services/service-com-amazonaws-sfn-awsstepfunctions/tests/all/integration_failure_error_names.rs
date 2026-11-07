// SPDX-License-Identifier: CC0-1.0

//! The ERROR NAME a `Task` state reports when the service integration it called
//! refused the call.
//!
//! The rule under test: the name is the called service as Step Functions names it in a
//! task error, a dot, and the EXCEPTION CLASS an AWS SDK of that service resolves the
//! refusal to — the modelled error's own shape name carrying the `Exception` suffix a
//! class name ends in, and the service's BASE SDK exception for a refusal its model
//! declares no error for. A definition's `Retry` and `Catch` match on that name, so it
//! is the name AWS reports and not the wire code the refusal travelled under.
//!
//! The table varies what the rule generalises over: which service was called, whether
//! its model declares an error for the refusal, and whether the shape name already
//! carries the suffix. Each service is mounted on the one engine the router below
//! builds, so every row's name comes from the refusal that service's own handler
//! really answered.
//!
//! The assembled router's own suite pins the same rule over the services it mounts
//! (`core/router/tests/all/sfn_integration_error_names.rs`); this one is the owning
//! crate's, and the harness it stands up is the one a sibling suite of this crate drives
//! for a task whose integration call is really served.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// Step Functions, Amazon SQS and DynamoDB on ONE engine, the shape
/// `core/router/src/lib.rs` mounts them in, so a task's call is served by the same
/// store this suite's queues were created in.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a sibling
/// suite that needs a task whose integration call is really served drives THIS harness
/// rather than standing up a second copy of it. Each service claims only the requests
/// scoped to its own signing name, so a suite that uses two of the three is routed no
/// differently for the third being mounted.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", &host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one Step Functions operation, requiring a 200.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.rpc("states", "AWSStepFunctions", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// Call one Amazon SQS operation, requiring a 200.
    pub(crate) async fn sqs(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.rpc("sqs", "AmazonSQS", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }
}

/// A queue called `name`, and its url. `pub(crate)` for the reason [`Emulator`] is.
pub(crate) async fn given_queue(emulator: &Emulator, name: &str, attributes: Value) -> String {
    let created = emulator
        .sqs(
            "CreateQueue",
            &json!({ "QueueName": name, "Attributes": attributes }),
        )
        .await;
    created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {created}"))
        .to_string()
}

/// A one-`Task` state machine calling `resource` with `parameters`, and an execution of
/// it described once it has settled.
async fn failed_task(
    emulator: &Emulator,
    name: &str,
    resource: &str,
    parameters: &Value,
) -> Value {
    let definition = json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
    .to_string();
    let created = emulator
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition,
                "roleArn": ROLE_ARN,
                "type": "STANDARD",
            }),
        )
        .await;
    let machine = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no stateMachineArn in {created}"));
    let started = emulator
        .sfn(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "once", "input": "{}" }),
        )
        .await;
    let execution = started["executionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no executionArn in {started}"))
        .to_string();
    ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
    emulator
        .sfn("DescribeExecution", &json!({ "executionArn": execution }))
        .await
}

/// One row: a call one service refuses, and the name the task must report it under.
struct Row {
    case: &'static str,
    /// The state machine's name for the row, which has to be unique in the account.
    machine: &'static str,
    resource: &'static str,
    parameters: Value,
    error: &'static str,
}

/// The name a refused integration call is reported under, over calls to two services
/// and over both halves of the rule.
#[tokio::test]
async fn a_refused_integration_call_is_named_after_the_services_sdk_exception() {
    let emulator = Emulator::new();
    // A FIFO queue: "A FIFO queue name must end with the .fifo suffix", and a send to
    // one has to carry a `MessageGroupId`, which Amazon SQS declares no error shape for.
    let fifo = given_queue(&emulator, "orders.fifo", json!({ "FifoQueue": "true" })).await;
    let plain = given_queue(&emulator, "orders", json!({})).await;
    // A url addressing a queue nobody created, which is the modelled `QueueDoesNotExist`.
    let absent = fifo.replace("orders.fifo", "nowhere");

    let rows = vec![
        Row {
            case: "a queue the account does not hold",
            machine: "sends-nowhere",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": &absent, "MessageBody": "hello" }),
            // `QueueDoesNotExist` is the error SQS's model declares
            // (`smithy_query(query="operation",operation="SendMessage")`), and an SDK's
            // class for it carries the suffix the shape name does not.
            error: "SQS.QueueDoesNotExistException",
        },
        Row {
            case: "a message body carrying a character SQS rejects",
            machine: "sends-a-rejected-body",
            resource: "arn:aws:states:::sqs:sendMessage",
            // "If a message contains characters outside the allowed set, Amazon SQS
            // rejects the message and returns an InvalidMessageContents error": another
            // error of the same model, and one the issue did not cite.
            parameters: json!({ "QueueUrl": &plain, "MessageBody": "a\u{0}b" }),
            error: "SQS.InvalidMessageContentsException",
        },
        Row {
            case: "a FIFO queue and no MessageGroupId",
            machine: "sends-without-a-group",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": &fifo, "MessageBody": "hello" }),
            // Amazon SQS's model declares no error shape for a parameter left out, so an
            // SDK resolves it to the service's base exception.
            error: "SQS.AmazonSQSException",
        },
        Row {
            case: "a table the account does not hold",
            machine: "writes-nowhere",
            resource: "arn:aws:states:::dynamodb:putItem",
            parameters: json!({
                "TableName": "nowhere",
                "Item": { "pk": { "S": "a" } },
            }),
            // DynamoDB's model spells this error's shape with the suffix already, so the
            // class is the shape name itself and the suffix is not doubled.
            error: "DynamoDB.ResourceNotFoundException",
        },
    ];

    for row in rows {
        let Row {
            case,
            machine,
            resource,
            parameters,
            error,
        } = row;
        let described = failed_task(&emulator, machine, resource, &parameters).await;
        assert_eq!(
            described["status"].as_str(),
            Some("FAILED"),
            "case {case:?}: expected the refused call to fail the execution, got {described}"
        );
        assert_eq!(
            described["error"].as_str(),
            Some(error),
            "case {case:?}: expected error {error:?}, got {:?} with cause {:?}",
            described["error"].as_str(),
            described["cause"].as_str()
        );
    }
}
