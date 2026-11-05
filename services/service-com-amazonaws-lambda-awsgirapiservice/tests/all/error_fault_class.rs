// SPDX-License-Identifier: CC0-1.0

//! Every failure this service answers names the class of its fault, end to end over HTTP.
//!
//! The rule under test: an error body of this service carries `Type` — "The exception
//! type", the member every error shape of the model declares — and what it names is whose
//! fault the failure is: `User` for a request the caller can fix, `Service` for a failure
//! inside the service. The model classifies each error shape itself (an `@error("client")`
//! shape is answered at a 4xx status and an `@error("server")` one at a 5xx), so the class
//! follows the status class and nothing of the request. A success carries no such member
//! at all.
//!
//! The rows differ along what the rule generalises over: the status of the refusal (each
//! one this service's modelled client errors are answered at), the operation that answers
//! it, the error code it carries, whether the refusal came from a handler or from the
//! boundary walk that refuses a request before any handler runs, and which of this
//! service's two serving seams answered it — the wrapper every generated operation is
//! served behind, and the hand-written entry that serves the one operation whose output
//! binds a `@streaming` union.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::service::fault_class::{
    SERVICE_FAULT, USER_FAULT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `TagResource` and `ListTags` are bound.
const TAGS_URI: &str = "/2017-03-31/tags";

/// The member the class travels in.
const FAULT_CLASS: &str = "Type";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries.
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// One service mounted on its own engine.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request and answer its status and body.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    dispatch(router, method, uri, &payload, &[]).await
}

/// Dispatch one request with the bytes and headers given, and answer its status and body.
///
/// The response-streaming route binds its `Payload` as the `@httpPayload`, so a row that
/// drives it states the body as BYTES — which is also how a body that is not JSON at all
/// reaches the handler — and names its invocation type in the modelled header.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    payload: &[u8],
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(payload.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answer = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, answer)
}

/// The function every row refuses something about.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..24u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let (status, body) = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction {name:?} must succeed, got {status} with {body}"
    );
}

/// One row driving the response-streaming entry: how the request addresses the function,
/// what it carries, and the failure it must be answered with.
struct StreamCase {
    name: &'static str,
    /// The `{FunctionName}` label the request addresses.
    reference: String,
    /// The `@httpPayload` bytes, which may be no JSON event at all.
    payload: &'static [u8],
    /// The `X-Amz-Invocation-Type` header the request carries, if any.
    invocation_type: Option<&'static str>,
    /// The status the answer must carry.
    status: u16,
    /// The class that status puts on the body.
    class: &'static str,
}

/// One row: a request that is refused, and the status it is refused at.
struct Case {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    status: u16,
}

/// The requests. Between them they cover each status this service's modelled client
/// errors are answered at, refusals raised by a handler and by the boundary walk, and
/// several operations.
fn cases(function: &str) -> Vec<Case> {
    let arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}");
    vec![
        Case {
            name: "a tag key in the reserved namespace",
            method: "POST",
            uri: format!("{TAGS_URI}/{arn}"),
            body: json!({ "Tags": { "aws:created-by": "someone" } }),
            status: 400,
        },
        Case {
            name: "a qualified name on a function-level setting",
            method: "GET",
            uri: format!("/2019-09-30/functions/{function}:1/concurrency"),
            body: Value::Null,
            status: 400,
        },
        Case {
            name: "a reservation past the account's unreservable capacity",
            method: "PUT",
            uri: format!("/2017-10-31/functions/{function}/concurrency"),
            body: json!({ "ReservedConcurrentExecutions": 100_000 }),
            status: 400,
        },
        Case {
            name: "a value outside a modelled bound, refused before any handler runs",
            method: "PUT",
            uri: format!("/2017-10-31/functions/{function}/concurrency"),
            body: json!({ "ReservedConcurrentExecutions": -1 }),
            status: 400,
        },
        Case {
            name: "a function that does not exist",
            method: "GET",
            uri: format!("{FUNCTIONS_URI}/absent-function"),
            body: Value::Null,
            status: 404,
        },
        Case {
            name: "tags on a resource this account does not hold",
            method: "GET",
            uri: format!("{TAGS_URI}/arn:aws:lambda:{REGION}:{ACCOUNT}:function:absent-function"),
            body: Value::Null,
            status: 404,
        },
        Case {
            name: "a function name that is already taken",
            method: "POST",
            uri: FUNCTIONS_URI.to_string(),
            body: json!({
                "FunctionName": function,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": "" },
            }),
            status: 409,
        },
    ]
}

#[tokio::test]
async fn every_refusal_names_the_class_of_its_fault() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "fault-class-subject";
    create_function(&router, function).await;

    for case in cases(function) {
        let label = case.name;
        let (status, body) = call(&router, case.method, &case.uri, &case.body).await;
        assert_eq!(
            status, case.status,
            "case {label:?}: expected {}, got {status} with {body}",
            case.status
        );
        let named = body
            .as_object()
            .and_then(|object| object.get(FAULT_CLASS))
            .unwrap_or_else(|| panic!("case {label:?}: the refusal must carry {FAULT_CLASS}: {body}"));
        assert_eq!(
            named.as_str(),
            Some(USER_FAULT),
            "case {label:?}: a request the caller can fix is a {USER_FAULT} fault, got {named}"
        );
        // The class travels BESIDE the text, never instead of it.
        assert!(
            body.get("message")
                .or_else(|| body.get("Message"))
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty()),
            "case {label:?}: the refusal must still carry its message: {body}"
        );
    }
}

/// The OTHER serving seam of this service: `InvokeWithResponseStream`, whose
/// `@streaming`-union output means it is served by its own hand-written entry rather than
/// behind the wrapper every generated operation is served behind. Its errors are error
/// shapes of this model like any other, so they name the class of their fault on the same
/// terms — a 4xx the caller can fix is a `User` fault, and the 5xx this emulator answers
/// for a run it cannot perform is a `Service` one.
///
/// The rows differ along what the rule generalises over: the status, the error code,
/// whether the refusal was raised by the handler or by the boundary walk the entry drives
/// before it, and the class the status puts on it.
#[tokio::test]
async fn the_response_streaming_entry_names_the_class_of_its_fault() {
    /// How long past a due instant this test calls. The transition is owed by the CLOCK, so
    /// nothing here sleeps on the emulator's behalf.
    const SLACK: Duration = Duration::from_millis(100);

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "fault-class-stream";
    create_function(&router, function).await;

    /// The route `InvokeWithResponseStream` is bound to, for the function `reference`.
    fn invocations(reference: &str) -> String {
        format!("/2021-11-15/functions/{reference}/response-streaming-invocations")
    }

    // Inside the provisioning window the invocation is refused on the function's state
    // ("During this time, you can't invoke or modify the function"), which is the one row
    // that has to run before the clock makes the function active.
    let pending = dispatch(&router, "POST", &invocations(function), b"{}", &[]).await;
    assert_eq!(
        pending.0, 409,
        "case an invocation inside the provisioning window: expected 409, got {} with {}",
        pending.0, pending.1
    );
    assert_eq!(
        pending.1.get(FAULT_CLASS).and_then(Value::as_str),
        Some(USER_FAULT),
        "case an invocation inside the provisioning window: expected the class {USER_FAULT}, \
         got {}",
        pending.1
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    let rows = vec![
        StreamCase {
            name: "a response-streaming invocation of a function that does not exist",
            reference: "absent-stream-function".to_string(),
            payload: b"{}",
            invocation_type: None,
            status: 404,
            class: USER_FAULT,
        },
        StreamCase {
            name: "a payload that is not the JSON event the function takes",
            reference: function.to_string(),
            payload: b"not-json",
            invocation_type: None,
            status: 400,
            class: USER_FAULT,
        },
        StreamCase {
            // Refused by the boundary walk the entry drives against the model, before the
            // handler reads anything: `ResponseStreamingInvocationType` is
            // `RequestResponse | DryRun`.
            name: "an invocation type outside the modelled enum",
            reference: function.to_string(),
            payload: b"{}",
            invocation_type: Some("Event"),
            status: 400,
            class: USER_FAULT,
        },
        StreamCase {
            // The run itself: this suite's deployment package is not a readable archive,
            // which is a failure inside the service and names the other class.
            name: "the run of a deployment package that cannot be read",
            reference: function.to_string(),
            payload: b"{}",
            invocation_type: Some("RequestResponse"),
            status: 502,
            class: SERVICE_FAULT,
        },
    ];

    for case in rows {
        let label = case.name;
        let headers: Vec<(&str, &str)> = case
            .invocation_type
            .map(|value| vec![("x-amz-invocation-type", value)])
            .unwrap_or_default();
        let (answered, body) = dispatch(
            &router,
            "POST",
            &invocations(&case.reference),
            case.payload,
            &headers,
        )
        .await;
        assert_eq!(
            answered, case.status,
            "case {label:?}: expected {}, got {answered} with {body}",
            case.status
        );
        assert_eq!(
            body.get(FAULT_CLASS).and_then(Value::as_str),
            Some(case.class),
            "case {label:?}: expected the class {}, got {body}",
            case.class
        );
        // The class travels BESIDE the text, never instead of it.
        assert!(
            body.get("message")
                .or_else(|| body.get("Message"))
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty()),
            "case {label:?}: the refusal must still carry its message: {body}"
        );
    }

    // And a success of the same entry names no class at all: a dry run reports no failure.
    let (dry_run, body) = dispatch(
        &router,
        "POST",
        &invocations(function),
        b"{}",
        &[("x-amz-invocation-type", "DryRun")],
    )
    .await;
    assert_eq!(
        dry_run, 204,
        "case a dry run: expected the documented dry-run status, got {dry_run} with {body}"
    );
    assert!(
        body.is_null(),
        "case a dry run: nothing ran, so the answer carries no body at all, got {body}"
    );
}

/// A success is not a failure: nothing adds a fault class to a body this service answered
/// a request with.
#[tokio::test]
async fn a_success_names_no_fault_class() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "fault-class-success";
    create_function(&router, function).await;

    // Each row names a member that answer must report, so a body that lost its own
    // members would fail here rather than passing for carrying no fault class.
    for (label, method, uri, reported) in [
        (
            "the account settings",
            "GET",
            "/2016-08-19/account-settings".to_string(),
            "AccountUsage",
        ),
        (
            "the function's concurrency",
            "GET",
            format!("/2019-09-30/functions/{function}/concurrency"),
            "ReservedConcurrentExecutions",
        ),
        (
            "the function itself",
            "GET",
            format!("{FUNCTIONS_URI}/{function}"),
            "Configuration",
        ),
        (
            "the function's tags",
            "GET",
            format!("{TAGS_URI}/arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}"),
            "Tags",
        ),
    ] {
        let (status, body) = call(&router, method, &uri, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {body}"
        );
        assert!(
            body.get(FAULT_CLASS).is_none(),
            "case {label:?}: a success must carry no {FAULT_CLASS} member: {body}"
        );
        assert!(
            body.as_object()
                .is_some_and(|object| object.contains_key(reported)),
            "case {label:?}: the answer must still report {reported}: {body}"
        );
    }
}
