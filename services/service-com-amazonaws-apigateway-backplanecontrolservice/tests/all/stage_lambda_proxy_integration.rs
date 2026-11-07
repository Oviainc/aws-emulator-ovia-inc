// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS_PROXY` (Lambda proxy) integration of a deployed stage:
//! the whole path a client takes from the stage invoke URL through the function to the item
//! the function writes, across API Gateway, AWS Lambda and Amazon DynamoDB on one engine.
//!
//! The rule under test: a deployed method whose integration is of type `AWS_PROXY` is served
//! by INVOKING the Lambda function its `uri` names, with the whole method request handed over
//! as one event ("the client request passed through as-is", `IntegrationType`), and the method
//! response is the envelope the function RETURNED — its `statusCode`, `headers`,
//! `multiValueHeaders`, `body` and `isBase64Encoded` are the status, headers and body the
//! caller is served. A return value that is not that envelope is a backend answer the gateway
//! cannot serve, so the caller is answered `502` with the fixed `{"message":"Internal server
//! error"}`; and whether the invocation happens at all is the FUNCTION's own resource-based
//! policy, which has to grant `apigateway.amazonaws.com` `lambda:InvokeFunction` for the ARN
//! of the method being invoked — a method the grant's `SourceArn` does not cover is never
//! invoked and is answered through the api's `API_CONFIGURATION_ERROR` gateway response
//! (`500`).
//!
//! The rows span what the rule generalises over: the FORM of the envelope (a status as a
//! number and as numeric text, a body that is text and one that is base64, a response with no
//! body, single- and multi-valued headers, a content type the envelope names), the VERB and
//! the shape of the request (a path parameter, a repeated query parameter, a repeated header,
//! a payload), and each way a call earns no envelope at all (a return value with no
//! `statusCode`, a handler that raised, and a method the function's policy does not cover).
//!
//! Only a cross-service suite can pin this: the function and its policy are AWS Lambda's, the
//! item the handler writes is Amazon DynamoDB's, and the api, the stage and the integration
//! are API Gateway's. The item is what tells an invocation that HAPPENED from one that did
//! not, which is the whole of the resource-policy row: the denied method's own marker must be
//! absent afterwards, while the malformed-envelope row's must be there, because that handler
//! ran before returning what it returned.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The stage variable the handler echoes, and the value this stage gives it: what proves the
/// INVOKED stage's own map reaches the event.
const MARKER_VARIABLE: &str = "marker";
const MARKER_VALUE: &str = "live-marker";

/// The table the handler writes the marker item into.
const TABLE: &str = "proxy-marks";

/// The function every method of the api is integrated with.
const FUNCTION: &str = "proxy-handler";

/// The role a created function names. No IAM store is mounted here, so it is a value the
/// create carries rather than a record this suite relies on.
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";

/// The handler the deployment package declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for another service's clock-owed transition (a created function
/// becoming `Active`, a created table becoming `ACTIVE`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The status and body an uncustomized `API_CONFIGURATION_ERROR` gateway response carries,
/// which is what a call whose integration could not be performed is answered with.
const CONFIGURATION_ERROR: (u16, &str) = (500, r#"{"message":"Internal server error"}"#);

/// The status and body API Gateway answers when the integration's backend answered something
/// it cannot turn into a response — recorded behaviour of the `execute-api` endpoint, whose
/// body is fixed text for the class.
const BAD_GATEWAY: (u16, &str) = (502, r#"{"message":"Internal server error"}"#);

/// The status and body an uncustomized `INTEGRATION_TIMEOUT` gateway response carries, which
/// is what a call whose backend had not answered by the integration's own `timeoutInMillis`
/// is answered with.
const INTEGRATION_TIMEOUT: (u16, &str) = (504, r#"{"message":"Endpoint request timed out"}"#);

/// The `timeoutInMillis` the slow method's integration carries: the lowest value
/// `Integration$timeoutInMillis` admits ("custom timeout between 50 and 29,000
/// milliseconds"), so the row below does not wait on the default 29 s to observe the bound.
const SHORT_TIMEOUT_MILLIS: u32 = 50;

/// The ONE handler every method is integrated with: it chooses what to return from the
/// `case` path parameter the event carries, so one deployment package serves every row, and
/// the rows differ in the envelope it returns rather than in which function answered.
///
/// Every branch that is meant to RUN writes its own marker item first, keyed by the path the
/// stage answered at, so an invocation that happened is told from one that did not.
const HANDLER_SOURCE: &str = r#"
import json
import os
import time

import boto3


def _mark(event):
    boto3.client("dynamodb").put_item(
        TableName=os.environ["TABLE"],
        Item={
            "pk": {"S": event["requestContext"]["path"]},
            "stage": {"S": event["requestContext"]["stage"]},
            "marker": {"S": (event.get("stageVariables") or {}).get("marker", "NONE")},
        },
    )


def lambda_handler(event, context):
    which = (event.get("pathParameters") or {}).get("case", "echo")
    if which == "created":
        return {
            "statusCode": "201",
            "headers": {"x-made": "yes"},
            "multiValueHeaders": {"set-cookie": ["a=1", "b=2"]},
            "body": "made",
        }
    if which == "empty":
        return {"statusCode": 204}
    if which == "binary":
        return {
            "statusCode": 200,
            "headers": {"Content-Type": "application/octet-stream"},
            "body": "AAEC/Q==",
            "isBase64Encoded": True,
        }
    if which == "raiser":
        raise ValueError("the handler itself failed")
    if which == "slow":
        time.sleep(2)
        return {"statusCode": 200, "body": "late"}
    _mark(event)
    if which == "malformed":
        return {"message": "this is not a proxy response"}
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json", "x-handler": which},
        "body": json.dumps(
            {
                "resource": event["resource"],
                "path": event["path"],
                "httpMethod": event["httpMethod"],
                "case": which,
                "pathParameters": event.get("pathParameters"),
                "query": event.get("queryStringParameters"),
                "multiQuery": event.get("multiValueQueryStringParameters"),
                "trace": event.get("multiValueHeaders", {}).get("x-trace"),
                "stage": event["requestContext"]["stage"],
                "apiId": event["requestContext"]["apiId"],
                "requestId": event["requestContext"]["requestId"],
                "contextPath": event["requestContext"]["path"],
                "marker": event.get("stageVariables", {}).get("marker"),
                "body": event.get("body"),
                "isBase64Encoded": event.get("isBase64Encoded"),
            }
        ),
    }
"#;

/// One answered request: its status, its headers and its body verbatim.
struct Answered {
    status: u16,
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl Answered {
    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is not JSON at all.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// Every value the response carries under the header `name`, in order.
    fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: the control-plane calls carry a credential
    // scope so the router can name the target service, and the invokes carry no credential at
    // all, as a client of a published invoke URL sends none.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect(),
        body,
    }
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answered.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire, answering its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// One Amazon DynamoDB call over its `awsJson1_0` wire, answering its status and parsed body.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let answered = dispatch(
        router,
        "POST",
        &format!("dynamodb.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &format!("DynamoDB_20120810.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    (answered.status, answered.json())
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com`, which is the endpoint
/// `RestApi$disableExecuteApiEndpoint` documents — carrying no credential at all, as a client
/// of a public method sends none.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answered {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        headers,
        Bytes::from(body.to_string()),
    )
    .await
}

/// The same request, addressed the way a client that cannot resolve that host reaches this
/// emulator: the data plane named in the PATH, ahead of the api id and the stage.
async fn invoke_path_style(
    router: &MultiServiceRouter,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answered {
    dispatch(
        router,
        verb,
        &format!("apigateway.{REGION}.amazonaws.com"),
        &format!("/_aws/execute-api/{api}/{STAGE}{path}"),
        headers,
        Bytes::from(body.to_string()),
    )
    .await
}

/// A .zip deployment package holding `source` as the module the handler names, written in
/// memory the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form every AWS request id carries, which
/// is what the event's `requestContext.requestId` must name the request it serves by.
fn is_request_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
///
/// The span a created function spends `Pending` is that service's own, so nothing here
/// sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter) {
    let looks = WAIT.as_millis() / LOOK.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (_, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup CreateFunction: the function never became Active, last read {last}");
}

/// Poll `DescribeTable` until DynamoDB first reports the table `ACTIVE`, which is when it
/// serves the writes the handler makes ("you can perform read and write operations only on an
/// ACTIVE table").
async fn wait_until_table_active(router: &MultiServiceRouter) {
    let looks = WAIT.as_millis() / LOOK.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (_, described) = dynamodb(router, "DescribeTable", json!({ "TableName": TABLE })).await;
        let status = described
            .pointer("/Table/TableStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == "ACTIVE" {
            return;
        }
        last = described;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup CreateTable: the table never became ACTIVE, last read {last}");
}

/// The marker item the handler wrote for the path `pk`, or `None` when nothing is stored
/// under it — which is what an invocation that never happened leaves behind.
async fn marker(router: &MultiServiceRouter, case: &str, pk: &str) -> Option<Value> {
    let (status, read) = dynamodb(
        router,
        "GetItem",
        json!({ "TableName": TABLE, "Key": { "pk": { "S": pk } }, "ConsistentRead": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetItem for {pk:?}, got {status} with {read}"
    );
    read.get("Item").cloned()
}

/// What the whole setup answers with: the api id of the deployed api.
async fn deployed(router: &MultiServiceRouter) -> String {
    // The table the handler writes, and the function that writes it.
    let (status, created) = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateTable: expected HTTP 200, got {status} with {created}"
    );

    let (status, function) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Environment": { "Variables": { "TABLE": TABLE } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected HTTP 201, got {status} with {function}"
    );
    let function_arn = member("CreateFunction", &function, "FunctionArn");

    // The api: `/open/{case}` reachable by GET and POST, and `/closed/{case}` by GET, both
    // in front of the same function. The two differ in nothing but their path, which is what
    // the function's own grant below is scoped by.
    let api = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "lambda-proxy-integration" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api, "id");
    let resources = apigw(
        router,
        "GetResources",
        "GET",
        &format!("/restapis/{api}/resources"),
        None,
        200,
    )
    .await;
    let root = resources
        .get("item")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .map(|item| member("GetResources", item, "id"))
        .expect("the api's root resource");

    for parent_part in ["open", "closed", "any", "slow"] {
        let parent = apigw(
            router,
            "CreateResource",
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": parent_part })),
            201,
        )
        .await;
        let parent = member("CreateResource", &parent, "id");
        // `/any` is reached through a GREEDY path parameter and an `ANY` method, the pair a
        // proxy integration is usually put behind; the other two name one segment and the
        // verbs explicitly.
        let leaf_part = if parent_part == "any" { "{rest+}" } else { "{case}" };
        let leaf = apigw(
            router,
            "CreateResource",
            "POST",
            &format!("/restapis/{api}/resources/{parent}"),
            Some(json!({ "pathPart": leaf_part })),
            201,
        )
        .await;
        let leaf = member("CreateResource", &leaf, "id");
        let verbs: &[&str] = match parent_part {
            "open" => &["GET", "POST"],
            "any" => &["ANY"],
            _ => &["GET"],
        };
        for verb in verbs {
            apigw(
                router,
                "PutMethod",
                "PUT",
                &format!("/restapis/{api}/resources/{leaf}/methods/{verb}"),
                Some(json!({ "authorizationType": "NONE" })),
                201,
            )
            .await;
            // `/slow`'s integration carries the SHORTEST wait the member admits, so the row
            // that observes the integration timeout does not sit on the 29 s default; every
            // other one is stored with no `timeoutInMillis` of its own and therefore gets
            // that default.
            let mut integration = json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {function_arn}/invocations"
                ),
            });
            if parent_part == "slow" {
                integration["timeoutInMillis"] = json!(SHORT_TIMEOUT_MILLIS);
            }
            apigw(
                router,
                "PutIntegration",
                "PUT",
                &format!("/restapis/{api}/resources/{leaf}/methods/{verb}/integration"),
                Some(integration),
                201,
            )
            .await;
        }
    }

    // "Grants a principal permission to use a function" (`AddPermission`): the grants that
    // admit the invocation, each scoped by `SourceArn` to the methods under ONE path of this
    // api — which is the whole of the per-method decision the rows below assert. Nothing
    // grants the methods under `/closed`.
    for granted_path in ["open", "any", "slow"] {
        let (status, granted) = lambda(
            router,
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/policy"),
            Some(json!({
                "StatementId": format!("apigateway-invoke-{granted_path}"),
                "Action": "lambda:InvokeFunction",
                "Principal": "apigateway.amazonaws.com",
                "SourceArn": format!(
                    "arn:aws:execute-api:{REGION}:{account}:{api}/*/*/{granted_path}/*",
                    account = engine::emulator_account_id(),
                ),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "setup AddPermission({granted_path}): expected HTTP 201, got {status} with \
             {granted}"
        );
    }

    let deployment = apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "description": "the snapshot the stage serves" })),
        201,
    )
    .await;
    let deployment = member("CreateDeployment", &deployment, "id");
    apigw(
        router,
        "CreateStage",
        "POST",
        &format!("/restapis/{api}/stages"),
        Some(json!({
            "stageName": STAGE,
            "deploymentId": deployment,
            "variables": { MARKER_VARIABLE: MARKER_VALUE },
        })),
        201,
    )
    .await;

    wait_until_active(router).await;
    wait_until_table_active(router).await;
    api
}

/// One row: the request it makes at the deployed stage, and the response the envelope the
/// function returned (or the refusal it earned) must be.
struct Row {
    case: &'static str,
    verb: &'static str,
    /// The path within the api, which also selects what the handler returns.
    path: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: &'static str,
    status: u16,
    /// The body the caller must be served, verbatim.
    expected_body: Option<&'static str>,
    /// The content type the response must carry.
    content_type: Option<&'static str>,
    /// Whether the marker item keyed by the path the stage answered at must be there
    /// afterwards: an invocation that happened wrote one, one that never happened did not.
    marked: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a well-formed envelope: the handler's status and body are the caller's",
            verb: "GET",
            path: "/open/echo?first=a&first=b&second=c",
            headers: &[("x-trace", "t1"), ("x-trace", "t2")],
            body: "",
            status: 200,
            expected_body: None,
            content_type: Some("application/json"),
            marked: true,
        },
        Row {
            case: "a payload the caller sent reaches the function and comes back",
            verb: "POST",
            path: "/open/echo",
            headers: &[("content-type", "application/json")],
            body: r#"{"in":1}"#,
            status: 200,
            expected_body: None,
            content_type: Some("application/json"),
            marked: true,
        },
        Row {
            case: "a status as numeric text, with single- and multi-valued headers",
            verb: "GET",
            path: "/open/created",
            headers: &[],
            body: "",
            status: 201,
            expected_body: Some("made"),
            content_type: Some("application/json"),
            marked: false,
        },
        Row {
            case: "an envelope with no body at all",
            verb: "GET",
            path: "/open/empty",
            headers: &[],
            body: "",
            status: 204,
            expected_body: Some(""),
            content_type: Some("application/json"),
            marked: false,
        },
        Row {
            case: "a base64 body, served as the bytes it encodes",
            verb: "GET",
            path: "/open/binary",
            headers: &[],
            body: "",
            status: 200,
            // The bytes `AAEC/Q==` encodes are not text, so the row asserts them below
            // rather than through this member.
            expected_body: None,
            content_type: Some("application/octet-stream"),
            marked: false,
        },
        Row {
            case: "a return value that is no envelope: the function ran and the stage answers 502",
            verb: "GET",
            path: "/open/malformed",
            headers: &[],
            body: "",
            status: BAD_GATEWAY.0,
            expected_body: Some(BAD_GATEWAY.1),
            content_type: Some("application/json"),
            marked: true,
        },
        Row {
            case: "a handler that raised: no envelope exists, so the stage answers 502",
            verb: "GET",
            path: "/open/raiser",
            headers: &[],
            body: "",
            status: BAD_GATEWAY.0,
            expected_body: Some(BAD_GATEWAY.1),
            content_type: Some("application/json"),
            marked: false,
        },
        Row {
            case: "a method the function's grant does not cover is never invoked",
            verb: "GET",
            path: "/closed/echo",
            headers: &[],
            body: "",
            status: CONFIGURATION_ERROR.0,
            expected_body: Some(CONFIGURATION_ERROR.1),
            content_type: Some("application/json"),
            marked: false,
        },
        Row {
            case: "a verb the ANY catch-all serves, behind a greedy path parameter",
            verb: "DELETE",
            path: "/any/deep/leaf",
            headers: &[],
            body: "",
            status: 200,
            expected_body: None,
            content_type: Some("application/json"),
            marked: true,
        },
        Row {
            case: "a function that had not answered by the integration's own timeout",
            verb: "GET",
            path: "/slow/slow",
            headers: &[],
            body: "",
            status: INTEGRATION_TIMEOUT.0,
            expected_body: Some(INTEGRATION_TIMEOUT.1),
            content_type: Some("application/json"),
            // The handler sleeps instead of writing, so there is no marker either way: what
            // this row asserts is that the CALLER is answered rather than held open.
            marked: false,
        },
    ]
}

/// Every row: the stage answers each request with what the function's own envelope says, and
/// only the invocations the function's policy admits happen at all.
///
/// Driven on a MULTI-THREADED runtime, as the assembled emulator serves a request: the chain
/// this rule needs is three services deep — the stage's request awaits the invocation, whose
/// handler makes its own call back into this process — so the test runs it the way the server
/// runs it rather than on the single-threaded default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lambda_proxy_integration_serves_the_envelope_the_function_returned() {
    let router = emulator();
    let api = deployed(&router).await;

    for row in rows() {
        let case = row.case;
        let answered = invoke(&router, &api, row.verb, row.path, row.headers, row.body).await;
        assert_eq!(
            answered.status,
            row.status,
            "case {case:?}: expected HTTP {}, got {} with {:?}",
            row.status,
            answered.status,
            answered.text()
        );
        if let Some(expected) = row.expected_body {
            assert_eq!(
                answered.text(),
                expected,
                "case {case:?}: expected the body {expected:?}, got {:?}",
                answered.text()
            );
        }
        if let Some(content_type) = row.content_type {
            assert_eq!(
                answered.header_values("content-type"),
                vec![content_type],
                "case {case:?}: expected the content type {content_type:?}, got {:?}",
                answered.header_values("content-type")
            );
        }
        // The marker item keyed by the path the stage answered at: present exactly where the
        // handler ran.
        let pk = format!(
            "/{STAGE}{path}",
            path = row.path.split('?').next().unwrap_or(row.path)
        );
        let held = marker(&router, case, &pk).await;
        assert_eq!(
            held.is_some(),
            row.marked,
            "case {case:?}: expected the handler's marker item under {pk:?} to be \
             {}, got {held:?}",
            if row.marked { "written" } else { "absent" }
        );
        if row.marked {
            let item = held.unwrap_or(Value::Null);
            assert_eq!(
                item.pointer("/marker/S").and_then(Value::as_str),
                Some(MARKER_VALUE),
                "case {case:?}: the invoked stage's own variables must reach the event, got \
                 {item}"
            );
            assert_eq!(
                item.pointer("/stage/S").and_then(Value::as_str),
                Some(STAGE),
                "case {case:?}: the event's request context must name the stage, got {item}"
            );
        }
    }
}

/// The EVENT the function is handed is the whole method request: the row that echoes it back
/// asserts each member against what the caller actually sent.
#[tokio::test]
async fn the_event_the_function_is_handed_is_the_method_request() {
    let router = emulator();
    let api = deployed(&router).await;

    let case = "a GET carrying a path parameter, a repeated query parameter and a repeated header";
    let answered = invoke(
        &router,
        &api,
        "GET",
        "/open/echo?first=a&first=b&second=c",
        &[("x-trace", "t1"), ("x-trace", "t2")],
        "",
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let echoed = answered.json();
    // The request id is the one value of the event no caller supplied: it is minted per
    // call, so the row asserts its FORM and that it is not empty rather than a value.
    let reported = answered.header_values("x-amzn-requestid");
    let named = echoed
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected = json!({
        "resource": "/open/{case}",
        "path": "/open/echo",
        "httpMethod": "GET",
        "case": "echo",
        "pathParameters": { "case": "echo" },
        "query": { "first": "a", "second": "c" },
        "multiQuery": { "first": ["a", "b"], "second": ["c"] },
        "trace": ["t1", "t2"],
        "stage": STAGE,
        "apiId": api,
        "requestId": named.clone(),
        "contextPath": format!("/{STAGE}/open/echo"),
        "marker": MARKER_VALUE,
        "body": Value::Null,
        "isBase64Encoded": false,
    });
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the response must name the request it answered exactly once, got \
         {reported:?}"
    );
    assert!(
        is_request_id(&named),
        "case {case:?}: the event must name the request it is serving, in the 8-4-4-4-12 \
         form every AWS request id carries, got {named:?}"
    );
    assert_eq!(
        echoed, expected,
        "case {case:?}: expected the event members {expected}, got {echoed}"
    );

    let case = "a POST carrying a JSON payload";
    let answered = invoke(
        &router,
        &api,
        "POST",
        "/open/echo",
        &[("content-type", "application/json")],
        r#"{"in":1}"#,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let echoed = answered.json();
    assert_eq!(
        echoed.get("httpMethod").and_then(Value::as_str),
        Some("POST"),
        "case {case:?}: the event's httpMethod is the request's own, got {echoed}"
    );
    assert_eq!(
        echoed.get("body").and_then(Value::as_str),
        Some(r#"{"in":1}"#),
        "case {case:?}: the payload reaches the function as the event's body, got {echoed}"
    );
    assert_eq!(
        echoed.get("isBase64Encoded"),
        Some(&Value::Bool(false)),
        "case {case:?}: a text payload is not base64, got {echoed}"
    );
    assert_eq!(
        echoed.get("query"),
        Some(&Value::Null),
        "case {case:?}: a call with no query string carries none, got {echoed}"
    );

    // The GREEDY path parameter and the `ANY` catch-all: the event's `resource` is the
    // template the deployment matched, its `httpMethod` is the verb the REQUEST carried, and
    // the parameter takes every remaining segment.
    let case = "a DELETE the ANY method serves behind a greedy path parameter";
    let answered = invoke(&router, &api, "DELETE", "/any/deep/leaf", &[], "").await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let echoed = answered.json();
    assert_eq!(
        echoed.get("resource").and_then(Value::as_str),
        Some("/any/{rest+}"),
        "case {case:?}: the event's resource is the matched template, got {echoed}"
    );
    assert_eq!(
        echoed.get("httpMethod").and_then(Value::as_str),
        Some("DELETE"),
        "case {case:?}: the event's httpMethod is the request's own verb, got {echoed}"
    );
    assert_eq!(
        echoed.pointer("/pathParameters/rest").and_then(Value::as_str),
        Some("deep/leaf"),
        "case {case:?}: the greedy parameter takes every remaining segment, got {echoed}"
    );

    // The SAME call, addressed the way a client that cannot resolve the api's own invoke
    // host reaches this emulator: the answer is the same function's envelope, and the event
    // still names the api's own path rather than the addressing prefix the request arrived
    // on.
    let case = "the same POST addressed through the data plane's own path prefix";
    let answered = invoke_path_style(
        &router,
        &api,
        "POST",
        "/open/echo",
        &[("content-type", "application/json")],
        r#"{"in":2}"#,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let echoed = answered.json();
    assert_eq!(
        echoed.get("path").and_then(Value::as_str),
        Some("/open/echo"),
        "case {case:?}: the event's path is the path within the api, got {echoed}"
    );
    assert_eq!(
        echoed.get("body").and_then(Value::as_str),
        Some(r#"{"in":2}"#),
        "case {case:?}: the payload reaches the function as the event's body, got {echoed}"
    );
}

/// The envelope's HEADERS reach the caller: a single-valued one as itself, and a
/// multi-valued one as one header line per value — which is the only way a header the wire
/// cannot comma-join survives.
#[tokio::test]
async fn the_envelopes_headers_reach_the_caller() {
    let router = emulator();
    let api = deployed(&router).await;

    let case = "an envelope carrying both header maps";
    let answered = invoke(&router, &api, "GET", "/open/created", &[], "").await;
    assert_eq!(
        answered.status,
        201,
        "case {case:?}: expected HTTP 201, got {} with {:?}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.header_values("x-made"),
        vec!["yes"],
        "case {case:?}: the single-valued header reaches the caller, got {:?}",
        answered.header_values("x-made")
    );
    assert_eq!(
        answered.header_values("set-cookie"),
        vec!["a=1", "b=2"],
        "case {case:?}: every value of the multi-valued header reaches the caller, got {:?}",
        answered.header_values("set-cookie")
    );
}

/// A base64 envelope body is served as the BYTES it encodes, which is what
/// `isBase64Encoded` says of it.
#[tokio::test]
async fn a_base64_envelope_body_is_served_as_bytes() {
    let router = emulator();
    let api = deployed(&router).await;

    let case = "an envelope declaring its body base64";
    let answered = invoke(&router, &api, "GET", "/open/binary", &[], "").await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.body.as_ref(),
        &[0u8, 1, 2, 253],
        "case {case:?}: expected the bytes the envelope's base64 encodes, got {:?}",
        answered.body.as_ref()
    );
}
