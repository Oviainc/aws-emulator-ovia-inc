// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler calls a REST-BOUND service of this emulator through, end to
//! end over the wire: Lambda and Amazon API Gateway on ONE engine, a `python3.x` function
//! whose code does `import boto3` and calls the control plane, and what it did read back
//! where a client reads it.
//!
//! RULE: a managed runtime's SDK reaches every service this emulator serves, over that
//! service's OWN protocol. For a `restJson1` service that means each operation's input is
//! bound the way its model binds it — the method and URI of its `@http` trait, its
//! `@httpLabel` members in the path, its `@httpQuery` members in the query, the rest as the
//! JSON body under the names `@jsonName` gives them — and the answer is read back under the
//! output shape's own member names, whatever the wire called each one. A refusal is the
//! service's own: its error code and message, raised as `botocore.exceptions.ClientError`.
//!
//! The steps differ along what the rule generalises over: WHICH binding class the operation
//! uses (a body-only create, a list with no parameters at all, a read addressed by one path
//! label, a create addressed by two, a read whose output renames its list member on the
//! wire), and whether the call is served or refused. They run in ONE invocation, because each
//! step's input is the previous step's answer and because an invocation is the dearest thing
//! this target does; the last assertion reads the whole thing back through API Gateway's OWN
//! API, so a call that answered without doing the work fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `MAX_CONCURRENT_RUNS` invocations at once for the whole process.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{await_function_active, call, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Lambda and Amazon API Gateway over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the API it
/// creates is the API this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One API Gateway call made as a CLIENT does, to read back what the handler's calls did.
async fn apigateway(router: &MultiServiceRouter, case: &str, method: &str, uri: &str) -> Value {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/apigateway/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::new())
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
        "case {case:?}: expected HTTP 200 from {method} {uri}, got {status} ({parsed})"
    );
    parsed
}

/// The handler the suite runs: one step per binding class of the protocol, each reported
/// under its own name so what is asserted is what the SDK gave the guest.
///
/// Every step's parameters come from the previous step's ANSWER — the API's id, the root
/// resource's id — so the chain is what the service answered and not what the test knew.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("apigateway")
    steps = {}

    # A body-only create: no label, no query, every member in the JSON body.
    created = client.create_rest_api(name=event["name"], description="created by a handler")
    steps["created"] = {"id": created.get("id"), "name": created.get("name")}
    api_id = created["id"]

    # A list with no parameters at all, whose output renames its list member on the wire
    # (`items` travels as `item`).
    listed = client.get_rest_apis()
    steps["listed"] = {
        "ids": [api.get("id") for api in listed.get("items", [])],
        "wire_name_leaked": "item" in listed,
    }

    # A read addressed by ONE path label.
    read = client.get_rest_api(restApiId=api_id)
    steps["read"] = {"id": read.get("id"), "name": read.get("name")}

    # A list addressed by one path label, for the parent the next step creates under.
    resources = client.get_resources(restApiId=api_id)
    roots = [one for one in resources.get("items", []) if one.get("path") == "/"]
    steps["resources"] = {"paths": [one.get("path") for one in resources.get("items", [])]}

    # A create addressed by TWO path labels, carrying a body member beside them.
    resource = client.create_resource(
        restApiId=api_id, parentId=roots[0]["id"], pathPart="orders"
    )
    steps["resource"] = {"pathPart": resource.get("pathPart"), "path": resource.get("path")}

    # A refusal is the SERVICE's own: its code and its message reach the handler.
    try:
        client.get_rest_api(restApiId="nosuchapi")
    except botocore.exceptions.ClientError as error:
        steps["refused"] = {
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
        }
    else:
        steps["refused"] = None

    return {"api": api_id, "steps": steps}
"#;

/// Every binding class of the protocol, in one invocation, each step asserted on what the
/// guest read back — and the last assertion on what the SERVICE itself holds.
#[tokio::test]
async fn a_handler_calls_a_rest_bound_service_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler calling API Gateway through the runtime's SDK";
    let router = emulator();
    let caller = "rest-sdk-caller";
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": caller,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(&router, case, caller).await;

    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{caller}/invocations"),
        &json!({ "name": "orders-api" }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    let payload = answered.json();
    let steps = payload["steps"].clone();
    let api_id = payload["api"].as_str().unwrap_or_default().to_string();
    assert!(
        !api_id.is_empty(),
        "case {case:?}: expected the handler's own chain to run through, got {payload}"
    );

    // A body-only create: the answer carries the created API's own members.
    assert_eq!(
        steps["created"]["name"],
        json!("orders-api"),
        "case {case:?}: expected the created API's own members, got {steps}"
    );
    assert_eq!(
        steps["created"]["id"],
        json!(api_id.clone()),
        "case {case:?}: expected the create to answer the id the chain used, got {steps}"
    );

    // A list whose output member is renamed on the wire: what the guest reads is the
    // member's OWN name, and the wire's name is not left in the document.
    assert!(
        steps["listed"]["ids"]
            .as_array()
            .is_some_and(|ids| ids.contains(&json!(api_id.clone()))),
        "case {case:?}: expected the created API among the listed ones, got {steps}"
    );
    assert_eq!(
        steps["listed"]["wire_name_leaked"],
        json!(false),
        "case {case:?}: the wire's own name is not what the guest reads, got {steps}"
    );

    // A read addressed by one path label.
    assert_eq!(
        steps["read"]["name"],
        json!("orders-api"),
        "case {case:?}: expected the API the label addressed, got {steps}"
    );

    // A list addressed by one path label, and a create addressed by two.
    assert_eq!(
        steps["resources"]["paths"],
        json!(["/"]),
        "case {case:?}: expected the API's root resource, got {steps}"
    );
    assert_eq!(
        steps["resource"]["pathPart"],
        json!("orders"),
        "case {case:?}: expected the created resource's own members, got {steps}"
    );

    // A refusal is the service's own: its code and its message reach the handler.
    assert_eq!(
        steps["refused"]["code"],
        json!("NotFoundException"),
        "case {case:?}: expected the service's own error code, got {steps}"
    );
    assert!(
        steps["refused"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected the service's own message, got {steps}"
    );

    // What API GATEWAY itself holds: the work the calls reported is the work that happened.
    let held = apigateway(
        &router,
        case,
        "GET",
        &format!("/restapis/{api_id}/resources"),
    )
    .await;
    let paths: Vec<&str> = held["item"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .filter_map(|resource| resource["path"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        paths.contains(&"/orders"),
        "case {case:?}: expected the handler's resource to be held by the service, got {held}"
    );
}
