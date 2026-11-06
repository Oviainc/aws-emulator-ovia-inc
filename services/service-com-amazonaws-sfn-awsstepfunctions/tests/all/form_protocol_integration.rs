// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a `Task` state's integration with a service whose protocol
//! carries its request as a FORM (`awsQuery`), driven over both services' own wire
//! protocols through the production `MultiServiceRouter` with Step Functions and STS
//! mounted on ONE engine — the shape `core/router/src/lib.rs` mounts them in.
//!
//! The rule under test: the document a `Task` state built IS the invoked operation's
//! request, wherever that service's protocol puts it — and for an `awsQuery` service
//! that is a form, the action and the service's API version beside one field per
//! member of the input, with the operation's own members read back out of the XML
//! document the answer wraps them in. So what such a task reports is the invoked
//! service's OWN answer, identical to what a client of that service is told.
//!
//! The rows span what the rule generalises over: the integration FAMILY that names the
//! service, and the SHAPE of the request's members (none at all, and a member whose
//! value is one this protocol binds to one field). A member this emulator does not
//! bind to a form field is still not faked: the execution fails with ASL's
//! `States.Runtime` and a cause that says what was not sent.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// Step Functions' wire facts, from its model's own `service_info`.
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";
const SFN_SIGNING_NAME: &str = "states";
const SFN_JSON_VERSION: &str = "1.0";

/// STS's signing name and awsQuery API version
/// (`smithy_query(query="service_info",model_path="services/
/// service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/
/// com-amazonaws-sts-awssecuritytokenservicev20110615.json")`: `sigv4Name` `sts`,
/// version `2011-06-15`).
const STS_SIGNING_NAME: &str = "sts";
const STS_API_VERSION: &str = "2011-06-15";

/// Step Functions and STS over ONE engine, so the identity a task reads is the one STS
/// itself answers for the same caller.
struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// Call one Step Functions operation, requiring a 200.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SFN_SIGNING_NAME}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{SFN_SIGNING_NAME}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{SFN_JSON_VERSION}"),
            )
            .header("x-amz-target", format!("{SFN_TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// One awsQuery call to STS, the way a client of that service makes it: an
    /// `application/x-www-form-urlencoded` POST naming the action and the API version.
    async fn sts(&self, action: &str) -> String {
        let host = format!("{STS_SIGNING_NAME}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{STS_SIGNING_NAME}/\
             aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization)
            .body(Bytes::from(format!(
                "Action={action}&Version={STS_API_VERSION}"
            )))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from STS itself, got {status} with body {body:?}"
        );
        body
    }

    /// Create a state machine, start one execution of it, and report what it ended as.
    async fn run(&self, name: &str, definition: &Value) -> Value {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        self.sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await
    }
}

/// A one-state definition whose `Task` names `resource` and sends `parameters`.
fn task(resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Work",
        "States": {
            "Work": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
}

/// The text of the first `<name>` element of an XML body.
fn element(body: &str, name: &str) -> String {
    body.split_once(&format!("<{name}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
        .map(|(text, _)| text.to_string())
        .unwrap_or_else(|| panic!("expected a <{name}> element in {body:?}"))
}

/// A `Task` state whose `Resource` names an operation of an `awsQuery` service makes
/// that call, and reports the operation's own members.
///
/// The row set spans the integration families that can name such a service: the AWS
/// SDK form (`aws-sdk:<service>:<action>`), whose result is the operation's own
/// members, and the optimized form (`<namespace>:<action>`), whose result is the SDK
/// response — the same members beside the envelope the SDK adds.
#[tokio::test]
async fn a_task_state_calls_a_form_protocol_service_and_reports_its_answer() {
    let chain = Chain::new();

    // What STS itself answers the same caller, read as a client of that service reads
    // it: the identity every row below must report.
    let answered = chain.sts("GetCallerIdentity").await;
    let account = element(&answered, "Account");
    let arn = element(&answered, "Arn");
    let user_id = element(&answered, "UserId");

    let rows: &[(&str, &str, bool)] = &[
        (
            "the AWS SDK family reports the operation's own members",
            "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
            false,
        ),
        (
            "the optimized family reports the SDK response",
            "arn:aws:states:::sts:getCallerIdentity",
            true,
        ),
    ];
    for (index, (case, resource, envelope)) in rows.iter().enumerate() {
        let described = chain
            .run(&format!("form-{index}"), &task(resource, json!({})))
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let produced: Value = described
            .get("output")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the output is JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
        for (member, expected) in [
            ("Account", &account),
            ("Arn", &arn),
            ("UserId", &user_id),
        ] {
            assert_eq!(
                produced.get(member).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: {member} is the answer STS itself gave ({expected}), got \
                 {produced}"
            );
        }
        assert_eq!(
            produced.get("SdkHttpMetadata").is_some(),
            *envelope,
            "case {case:?}: the SDK response envelope belongs to the optimized family alone, \
             got {produced}"
        );
    }
}

/// A request member this emulator does not bind to a form field is not sent: the
/// execution fails with ASL's `States.Runtime` and a cause that says what was not run.
///
/// The rows differ along the kind of value that cannot be one field — a list and a
/// structure, each of which this protocol spreads over field names that are the
/// invoked shape's own.
#[tokio::test]
async fn a_member_no_form_field_carries_is_not_sent() {
    let chain = Chain::new();

    let rows: &[(&str, Value)] = &[
        ("a list member", json!({ "PolicyArns": ["arn:aws:iam::1:policy/p"] })),
        ("a structure member", json!({ "Tags": { "Key": "k" } })),
    ];
    for (index, (case, parameters)) in rows.iter().enumerate() {
        let described = chain
            .run(
                &format!("unbound-{index}"),
                &task(
                    "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    parameters.clone(),
                ),
            )
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("States.Runtime"),
            "case {case:?}: expected States.Runtime, got {described:?}"
        );
        assert!(
            described
                .get("cause")
                .and_then(Value::as_str)
                .is_some_and(|cause| cause.contains("form-field protocol")),
            "case {case:?}: the cause says what was not bound, got {described:?}"
        );
    }
}

/// A scalar member of a form request reaches the invoked operation as its own field,
/// whatever characters it carries: the delimiters of the form are escaped rather than
/// splitting one value into several.
///
/// The rows differ along what the value holds: a plain token, an ARN (whose `:` and
/// `/` are reserved in a URI), and a value carrying the form's own delimiters. Each is
/// a member STS's `GetCallerIdentity` does not model, so the call is still made and
/// answered — what is pinned is that the request was BUILT and sent rather than
/// refused.
#[tokio::test]
async fn a_scalar_member_is_one_field_whatever_it_carries() {
    let chain = Chain::new();

    let rows: &[(&str, Value)] = &[
        ("a plain token", json!("session-one")),
        ("an ARN", json!("arn:aws:iam::000000000000:role/Demo")),
        ("the form's own delimiters", json!("a&b=c+d%e")),
        ("a number", json!(900)),
        ("a boolean", json!(true)),
    ];
    for (index, (case, value)) in rows.iter().enumerate() {
        let described = chain
            .run(
                &format!("scalar-{index}"),
                &task(
                    "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    json!({ "RoleSessionName": value }),
                ),
            )
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: a scalar member goes on the wire as one field, got {described:?}"
        );
    }
}
