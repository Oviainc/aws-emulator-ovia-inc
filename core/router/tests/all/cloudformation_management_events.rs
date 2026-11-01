// SPDX-License-Identifier: CC0-1.0

//! CloudFormation control-plane calls in CloudTrail event history, across two services on
//! one engine.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service, so `cloudtrail:LookupEvents` reports the call under its own `eventName`
//! with the serving service's `eventSource` — carrying the parameters the request sent, the
//! values a mutating call answered as `responseElements` (null for a call that answered
//! nothing), the FAULT SHAPE's name as `errorCode` with the message the caller was answered
//! with (not always the code the caller saw: a query protocol renders the shape's query
//! error code on the wire), and as `requestID` the id the caller was answered under, which
//! is the join a client makes from a response it holds to the record of that very call.
//!
//! Only a cross-service suite can pin this: the record is written by the service that
//! SERVES the call (CloudFormation, awsQuery) and read back through CloudTrail
//! (awsJson1_1) — two services over ONE engine, as `core/router/src/lib.rs` mounts them.
//! The per-service half of the rule, over every kind of call this front door serves, is
//! `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/management_event_recording.rs`.
//!
//! The rows span what the rule generalises over: a refused call and a served one, over a
//! `RoleARN` the account's IAM records show CloudFormation cannot assume and one it can, so
//! both halves of the correlation (an error envelope's own `<RequestId>` and a success's
//! `ResponseMetadata/RequestId`) are joined to their records.
//!
//! Every call goes over the wire, on a router with request authentication off as every
//! suite in this crate runs it. Assembling the whole router costs stack (see
//! `boot_serve.rs`), so it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate states ONCE: the form-body percent-encoder and
// the element reader. Called rather than written again, as that module's header requires.
use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the records live in.
const REGION: &str = "us-west-2";

/// The access key id every call carries.
const ACCESS_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on a CloudFormation management event.
const CLOUDFORMATION_EVENT_SOURCE: &str = "cloudformation.amazonaws.com";

/// The template every stack here is created from: one resource of CloudFormation's OWN
/// type, so the create writes nothing outside CloudFormation's own store and the records
/// this suite reads are CloudFormation's alone.
const TEMPLATE: &str =
    r#"{"Resources":{"TheHandle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// What one call answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one request signed for `signing_name`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone())
        .header(
            // The signature is never checked: this router runs with authentication off.
            // What the scope carries is the key id and the Region a handler reads.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={ACCESS_KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsQuery call, form-encoded as an AWS SDK sends it.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        runtime,
        signing_name,
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// `cloudtrail:LookupEvents` filtered on one attribute, answering each event's
/// `CloudTrailEvent` document.
fn lookup(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    attribute_key: &str,
    attribute_value: &str,
) -> Vec<Value> {
    let answered = dispatch(
        router,
        runtime,
        "cloudtrail",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "CloudTrail_20131101.LookupEvents"),
        ],
        bytes::Bytes::from(
            json!({
                "LookupAttributes": [{
                    "AttributeKey": attribute_key,
                    "AttributeValue": attribute_value,
                }],
            })
            .to_string(),
        ),
    );
    assert_eq!(
        answered.status,
        200,
        "LookupEvents({attribute_key}={attribute_value}) must succeed, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {}", answered.text()))
        .iter()
        .map(|event| {
            let document = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
        })
        .collect()
}

/// A trust policy granting `principal` the right to assume the role.
fn trust(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// The record among `events` whose `requestID` is `request_id` — the join a client holding a
/// response makes.
fn joined_on<'a>(case: &str, events: &'a [Value], request_id: &str) -> &'a Value {
    let found: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("requestID") == Some(&json!(request_id)))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one record of request {request_id}, got {events:?}"
    );
    found[0]
}

#[test]
fn cloudformation_calls_are_in_cloudtrail_event_history() {
    std::thread::Builder::new()
        .name("cloudformation-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Two roles, written by the REAL `CreateRole` on the same engine: the one
            // CloudFormation may assume and the one it may not.
            let mut role_arns = Vec::new();
            for (name, principal) in [
                ("trail-untrusted-role", "lambda.amazonaws.com"),
                ("trail-trusted-role", "cloudformation.amazonaws.com"),
            ] {
                let created = query(
                    &router,
                    &runtime,
                    "iam",
                    "2010-05-08",
                    "CreateRole",
                    &[
                        ("RoleName", name),
                        ("AssumeRolePolicyDocument", &trust(principal)),
                    ],
                );
                assert_eq!(
                    created.status,
                    200,
                    "setup: CreateRole({name}) must succeed, got {} with {:?}",
                    created.status,
                    created.text()
                );
                role_arns.push(element(&created.text(), "Arn"));
            }
            let [untrusted, trusted] = <[String; 2]>::try_from(role_arns).expect("two roles");

            // The REFUSED create: CloudFormation answers the caller its own query error
            // code, and writes nothing.
            let refused = query(
                &router,
                &runtime,
                "cloudformation",
                "2010-05-15",
                "CreateStack",
                &[
                    ("StackName", "trail-refused-stack"),
                    ("TemplateBody", TEMPLATE),
                    ("RoleARN", &untrusted),
                ],
            );
            assert_eq!(
                refused.status,
                400,
                "a role CloudFormation cannot assume must be refused, got {} with {:?}",
                refused.status,
                refused.text()
            );
            let refused_body = refused.text();
            let answered_code = element(&refused_body, "Code");
            assert_eq!(
                answered_code, "ValidationError",
                "the caller sees the query error code, got {answered_code:?} in {refused_body:?}"
            );
            let refused_message = element(&refused_body, "Message");
            // The error envelope's own `<RequestId>`: the reader panics quoting the whole
            // answer when the envelope names no request, which is the failure to report.
            let refused_request_id = element(&refused_body, "RequestId");

            // The SERVED create, with the trusted role.
            let served = query(
                &router,
                &runtime,
                "cloudformation",
                "2010-05-15",
                "CreateStack",
                &[
                    ("StackName", "trail-served-stack"),
                    ("TemplateBody", TEMPLATE),
                    ("RoleARN", &trusted),
                ],
            );
            assert_eq!(
                served.status,
                200,
                "a role CloudFormation may assume must be accepted, got {} with {:?}",
                served.status,
                served.text()
            );
            let served_body = served.text();
            let stack_id = element(&served_body, "StackId");
            // The success's own id, which `ResponseMetadata` carries.
            let served_request_id = element(&served_body, "RequestId");

            // Both calls are in event history, found the way the check that filed this
            // issue finds them: a lookup by event source.
            let by_source = lookup(
                &router,
                &runtime,
                "EventSource",
                CLOUDFORMATION_EVENT_SOURCE,
            );
            assert_eq!(
                by_source.len(),
                2,
                "both CloudFormation calls are recorded under \
                 {CLOUDFORMATION_EVENT_SOURCE}, got {by_source:?}"
            );

            // The refusal's record, reached by the id the caller was answered under: the
            // FAULT SHAPE's name (not the wire code), the caller's own message, nothing
            // minted, and the role the call was made with.
            let refusal = joined_on("the refused create", &by_source, &refused_request_id);
            assert_eq!(
                refusal.get("eventName"),
                Some(&json!("CreateStack")),
                "the record names the operation, got {refusal}"
            );
            assert_eq!(
                refusal.get("errorCode"),
                Some(&json!("ValidationException")),
                "the trail records the fault shape name, got {refusal}"
            );
            assert_eq!(
                refusal.get("errorMessage"),
                Some(&json!(refused_message)),
                "the trail records the message the caller was answered with, got {refusal}"
            );
            assert_eq!(
                refusal.get("responseElements"),
                Some(&Value::Null),
                "a refused create minted nothing, got {refusal}"
            );
            assert_eq!(
                refusal.pointer("/requestParameters/roleARN"),
                Some(&json!(untrusted)),
                "the record names the role the call was made with, got {refusal}"
            );
            assert!(
                refusal
                    .pointer("/userIdentity/arn")
                    .and_then(Value::as_str)
                    .is_some_and(|arn| arn.contains(":iam::")),
                "the record names the IAM entity the caller is, got {refusal}"
            );

            // The served call's record: no error code, the stack id it answered, and the
            // Region it was served in.
            let acceptance = joined_on("the served create", &by_source, &served_request_id);
            assert_eq!(
                acceptance.get("errorCode"),
                None,
                "a served call carries no error code, got {acceptance}"
            );
            assert_eq!(
                acceptance.pointer("/responseElements/stackId"),
                Some(&json!(stack_id)),
                "the record carries the stack id the call answered, got {acceptance}"
            );
            assert_eq!(
                acceptance.pointer("/requestParameters/roleARN"),
                Some(&json!(trusted)),
                "the record names the role the call was made with, got {acceptance}"
            );
            assert_eq!(
                acceptance.get("awsRegion"),
                Some(&json!(REGION)),
                "the record is in the Region the call was served in, got {acceptance}"
            );
            assert_eq!(
                acceptance.get("readOnly"),
                Some(&json!(false)),
                "a create changes the account, got {acceptance}"
            );

            // And the same records are reachable by the other attribute a client looks up a
            // call by: the operation's own name.
            let by_name = lookup(&router, &runtime, "EventName", "CreateStack");
            let ids: Vec<&str> = by_name
                .iter()
                .filter_map(|event| event.get("requestID").and_then(Value::as_str))
                .collect();
            for request_id in [&refused_request_id, &served_request_id] {
                assert!(
                    ids.contains(&request_id.as_str()),
                    "a lookup by EventName reports the call answered as {request_id}, got {ids:?}"
                );
            }
        })
        .expect("spawn the CloudFormation management-event thread")
        .join()
        .expect("CloudFormation control-plane calls must be in CloudTrail event history");
}
