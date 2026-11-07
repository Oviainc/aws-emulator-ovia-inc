// SPDX-License-Identifier: CC0-1.0

//! A deployed method whose integration is `AWS_PROXY`: the whole path a client takes from
//! the stage invoke URL through the Lambda function to the DynamoDB item its handler wrote.
//!
//! RULE: a deployed method whose integration type is `AWS_PROXY` is served by INVOKING the
//! Lambda function its `uri` names — `IntegrationType` gives it as "for integrating the API
//! method request with the Lambda function-invoking action with the client request passed
//! through as-is" — handing the function the whole method request as one proxy event, and the
//! document the handler returned IS the method response: its `statusCode`, its `headers` and
//! `multiValueHeaders`, and its `body` (base64-decoded where `isBase64Encoded` says so),
//! mapped by nothing. A function that returned no such document is answered `502` with
//! `{"message":"Internal server error"}`, and an invocation the function's own resource
//! policy admits no `lambda:InvokeFunction` from this method for is never made — the call is
//! answered through the api's `API_CONFIGURATION_ERROR` gateway response (`500`), with the
//! handler never reached and so nothing written. All of that happens AFTER the method's
//! `apiKeyRequired` gate, so a request presenting no key the records associate with the stage
//! is `403` with the integration never invoked.
//!
//! Three services have to agree for the rule to hold — API Gateway holds the api, the stage
//! and the integration, Lambda holds and runs the function, DynamoDB serves the table the
//! handler writes — so all three are mounted on this suite's own engine, and every row
//! asserts BOTH halves: what the stage answered, and what the table holds afterwards read
//! through DynamoDB's own API. A call that answered without running the handler, or ran it
//! without answering, fails here.
//!
//! The rows differ along what the rule generalises over: the VERB the client sends (a `GET`
//! matched by the method's own verb, a `POST` and a `DELETE` matched by the `ANY` catch-all),
//! whether the resource path is literal or binds a path PARAMETER, whether the integration's
//! `uri` names the function by ARN or by name alone, whether the request carries a BODY and a
//! repeated query parameter, the shape of the document the handler returns (a JSON body, a
//! base64-encoded binary body with a declared media type and a repeated response header, a
//! document carrying no `statusCode`, a handler that raises), whether the function's resource
//! policy admits the invocation for that path, and whether the request presents an API key
//! the stage's usage plan associates with it.

use std::io::Write as _;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the Region the integration ARNs name.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The table the handler writes, and its one key attribute.
const TABLE: &str = "apigw-proxy-items";
const KEY_ATTRIBUTE: &str = "pk";

/// The function the `AWS_PROXY` integrations name.
const FUNCTION: &str = "apigw-proxy-handler";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The one key type a `UsagePlanKey` takes: "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// The stage variable the echoing row reads back out of the event.
const STAGE_VARIABLE: &str = "shelf";
const STAGE_VARIABLE_VALUE: &str = "front";

/// How long one invocation is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
/// The spans belong to Lambda and DynamoDB, so nothing here sleeps a fixed span for them.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// One answered request: its status, its headers and its body verbatim.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Answer {
    /// One response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// EVERY value the response carries under `name`.
    fn every_header(&self, name: &str) -> Vec<String> {
        self.headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_string)
            .collect()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// API Gateway, Lambda and DynamoDB over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the handler's own SDK call is dispatched back into this same router, so the
/// table it writes is the table this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
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
) -> Answer {
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
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One API Gateway control-plane call that has to answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
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
        answer.status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One Lambda `restJson1` call.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
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
    .await
}

/// One DynamoDB `awsJson1_0` call.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let target = format!("DynamoDB_20120810.{operation}");
    dispatch(
        router,
        "POST",
        &format!("dynamodb.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &target),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com` that
/// `RestApi$disableExecuteApiEndpoint` documents — carrying no credential at all, as a client
/// of a public method sends it.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        headers,
        body,
    )
    .await
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
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

/// The handler every row runs. The event's own query string selects which SHAPE of document
/// it returns, so one package serves every row — which is what the rule generalises over.
const HANDLER_SOURCE: &str = r#"
import base64
import json
import os

import boto3

TABLE = os.environ.get("TABLE", "")


def _query(event):
    return event.get("queryStringParameters") or {}


def _store(event):
    item_id = _query(event).get("id")
    if item_id and TABLE:
        boto3.client("dynamodb").put_item(
            TableName=TABLE,
            Item={"pk": {"S": item_id}, "source": {"S": "proxy-route"}},
        )
    return item_id


def lambda_handler(event, context):
    mode = _query(event).get("mode", "stored")
    if mode == "raise":
        raise RuntimeError("the handler refused to answer")
    item_id = _store(event)
    if mode == "malformed":
        return {"body": "no status code at all"}
    if mode == "binary":
        return {
            "statusCode": 201,
            "headers": {"Content-Type": "application/octet-stream"},
            "multiValueHeaders": {"X-Seen": ["one", "two"]},
            "body": base64.b64encode(bytes([0, 1, 2])).decode("ascii"),
            "isBase64Encoded": True,
        }
    if mode == "echo":
        reported = event.get("requestContext") or {}
        return {
            "statusCode": 200,
            "headers": {"Content-Type": "application/json"},
            "body": json.dumps(
                {
                    "httpMethod": event.get("httpMethod"),
                    "resource": event.get("resource"),
                    "path": event.get("path"),
                    "pathParameters": event.get("pathParameters"),
                    "query": _query(event),
                    "multiQuery": event.get("multiValueQueryStringParameters"),
                    "stageVariables": event.get("stageVariables"),
                    "requestBody": event.get("body"),
                    "isBase64Encoded": event.get("isBase64Encoded"),
                    "apiId": reported.get("apiId"),
                    "stage": reported.get("stage"),
                    "contextPath": reported.get("path"),
                    "resourceId": reported.get("resourceId"),
                    "headerNames": sorted(
                        [name.lower() for name in (event.get("headers") or {})]
                    ),
                }
            ),
        }
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"stored": item_id}),
    }
"#;

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.json().get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// Poll `DescribeTable` until DynamoDB first reports the table `ACTIVE` — "you can perform
/// read and write operations only on an ACTIVE table".
async fn wait_until_table_active(router: &MultiServiceRouter, table: &str) {
    for _ in 0..POLLS {
        let described = dynamodb(router, "DescribeTable", &json!({ "TableName": table })).await;
        let status = described
            .json()
            .get("Table")
            .and_then(|table| table.get("TableStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == "ACTIVE" {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: DynamoDB never reported {table} ACTIVE");
}

/// Whether the table holds an item under `id`, read through DynamoDB's own `GetItem`.
async fn item_exists(router: &MultiServiceRouter, id: &str) -> bool {
    let read = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": TABLE,
            "Key": { KEY_ATTRIBUTE: { "S": id } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "setup GetItem({id}): expected HTTP 200 from DynamoDB, got {} with {:?}",
        read.status,
        read.text()
    );
    read.json().get("Item").is_some_and(|item| item.is_object())
}

/// A child resource of `parent` named `part`, answering its id.
async fn resource(router: &MultiServiceRouter, api: &str, parent: &str, part: &str) -> String {
    let created = apigw(
        router,
        &format!("CreateResource({part})"),
        "POST",
        &format!("/restapis/{api}/resources/{parent}"),
        Some(json!({ "pathPart": part })),
        201,
    )
    .await;
    member(&format!("CreateResource({part})"), &created, "id")
}

/// One method on `resource` for `verb`, behind an `AWS_PROXY` integration naming `function`.
async fn proxy_method(
    router: &MultiServiceRouter,
    api: &str,
    resource: &str,
    verb: &str,
    function: &str,
    key_required: bool,
) {
    apigw(
        router,
        &format!("PutMethod({resource}/{verb})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
        Some(json!({ "authorizationType": "NONE", "apiKeyRequired": key_required })),
        201,
    )
    .await;
    apigw(
        router,
        &format!("PutIntegration({resource}/{verb})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function}/invocations"
            ),
        })),
        201,
    )
    .await;
}

/// One row: the request the client makes and the answer the records earn it.
struct Row {
    case: &'static str,
    verb: &'static str,
    /// The path within the stage, query string included.
    path: String,
    /// Whether the request presents the API key the usage plan associates with the stage.
    keyed: bool,
    body: &'static str,
    status: u16,
    /// The media type the answer must declare, when the row pins one.
    content_type: Option<&'static str>,
    /// The id the handler writes when it runs, and whether the table must hold it
    /// afterwards.
    item: Option<&'static str>,
    item_written: bool,
}

#[tokio::test]
async fn an_aws_proxy_method_is_served_by_the_function_its_uri_names() {
    let router = emulator();

    // The table the handler writes, and the function that writes it.
    let created = dynamodb(
        &router,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{ "AttributeName": KEY_ATTRIBUTE, "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "setup CreateTable: expected HTTP 200 from DynamoDB, got {} with {:?}",
        created.status,
        created.text()
    );
    wait_until_table_active(&router, TABLE).await;

    let function = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": { "Variables": { "TABLE": TABLE } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        function.status,
        201,
        "setup CreateFunction: expected HTTP 201 from Lambda, got {} with {:?}",
        function.status,
        function.text()
    );
    let function_arn = member("CreateFunction", &function.json(), "FunctionArn");
    wait_until_active(&router, FUNCTION).await;

    // The api: one literal resource behind a key-required method, one resource binding a
    // path parameter behind the `ANY` catch-all, and one the function's policy will NOT be
    // granted for. It declares `application/octet-stream` BINARY, which is what makes the
    // base64 body the handler returns under that media type reach the client as the bytes
    // it names: "the list of binary media types supported by the RestApi. By default, the
    // RestApi supports only UTF-8-encoded text payloads"
    // (`smithy_query(query="doc", shape="RestApi", member="binaryMediaTypes")`).
    let api = member(
        "CreateRestApi",
        &apigw(
            &router,
            "CreateRestApi",
            "POST",
            "/restapis",
            Some(json!({
                "name": "aws-proxy-integration",
                "binaryMediaTypes": ["application/octet-stream"],
            })),
            201,
        )
        .await,
        "id",
    );
    let root = member(
        "GetResources",
        &apigw(
            &router,
            "GetResources",
            "GET",
            &format!("/restapis/{api}/resources"),
            None,
            200,
        )
        .await
        .get("item")
        .and_then(|items| items.get(0).cloned())
        .unwrap_or(Value::Null),
        "id",
    );
    let items = resource(&router, &api, &root, "items").await;
    let things = resource(&router, &api, &root, "things").await;
    let one_thing = resource(&router, &api, &things, "{id}").await;
    let named = resource(&router, &api, &root, "named").await;
    let ungranted = resource(&router, &api, &root, "ungranted").await;
    proxy_method(&router, &api, &items, "GET", &function_arn, true).await;
    proxy_method(&router, &api, &one_thing, "ANY", &function_arn, false).await;
    // The same function, addressed by NAME alone rather than by ARN: the `uri` names one
    // function either way, so the grant is taken on the ARN Lambda resolves for it.
    proxy_method(&router, &api, &named, "GET", FUNCTION, false).await;
    proxy_method(&router, &api, &ungranted, "GET", &function_arn, false).await;

    apigw(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({
            "stageName": STAGE,
            "variables": { STAGE_VARIABLE: STAGE_VARIABLE_VALUE },
        })),
        201,
    )
    .await;

    // The grant that admits the invocation, narrowed by `aws:SourceArn` to the two paths the
    // rows reach — which is what leaves `/ungranted` ungranted on the SAME function.
    let account = engine::emulator_account_id();
    for (statement, path) in [
        ("items", "items"),
        ("things", "things/*"),
        ("named", "named"),
    ] {
        let granted = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/policy"),
            Some(json!({
                "StatementId": format!("apigw-{statement}"),
                "Action": "lambda:InvokeFunction",
                "Principal": "apigateway.amazonaws.com",
                "SourceArn": format!(
                    "arn:aws:execute-api:{REGION}:{account}:{api}/*/*/{path}"
                ),
            })),
        )
        .await;
        assert_eq!(
            granted.status,
            201,
            "setup AddPermission({statement}): expected HTTP 201 from Lambda, got {} with {:?}",
            granted.status,
            granted.text()
        );
    }

    // The API key the stage's usage plan associates with it, which the key-required method
    // admits only that key for.
    let key = apigw(
        &router,
        "CreateApiKey",
        "POST",
        "/apikeys",
        Some(json!({ "name": "proxy-key", "enabled": true })),
        201,
    )
    .await;
    let key_id = member("CreateApiKey", &key, "id");
    let key_value = member("CreateApiKey", &key, "value");
    let plan = apigw(
        &router,
        "CreateUsagePlan",
        "POST",
        "/usageplans",
        Some(json!({
            "name": "proxy-plan",
            "apiStages": [{ "apiId": api, "stage": STAGE }],
        })),
        201,
    )
    .await;
    let plan_id = member("CreateUsagePlan", &plan, "id");
    apigw(
        &router,
        "CreateUsagePlanKey",
        "POST",
        &format!("/usageplans/{plan_id}/keys"),
        Some(json!({ "keyId": key_id, "keyType": API_KEY_TYPE })),
        201,
    )
    .await;

    let rows = [
        Row {
            case: "a keyed GET of a literal path: the handler's own 200 and body",
            verb: "GET",
            path: "/items?id=withkey-1".to_string(),
            keyed: true,
            body: "",
            status: 200,
            content_type: Some("application/json"),
            item: Some("withkey-1"),
            item_written: true,
        },
        Row {
            case: "the same GET with no api key: refused before the integration, so nothing \
                   is written",
            verb: "GET",
            path: "/items?id=nokey-1".to_string(),
            keyed: false,
            body: "",
            status: 403,
            content_type: None,
            item: Some("nokey-1"),
            item_written: false,
        },
        Row {
            case: "a POST matched by the ANY catch-all, carrying a body and a repeated query \
                   parameter",
            verb: "POST",
            path: "/things/t-1?mode=echo&tag=a&tag=b".to_string(),
            keyed: false,
            body: r#"{"sent":true}"#,
            status: 200,
            content_type: Some("application/json"),
            item: None,
            item_written: false,
        },
        Row {
            case: "a DELETE matched by the same catch-all, which writes the item it names",
            verb: "DELETE",
            path: "/things/t-2?id=deleted-1".to_string(),
            keyed: false,
            body: "",
            status: 200,
            content_type: Some("application/json"),
            item: Some("deleted-1"),
            item_written: true,
        },
        Row {
            case: "a handler returning a base64 body with its own status and media type",
            verb: "GET",
            path: "/things/t-3?mode=binary&id=binary-1".to_string(),
            keyed: false,
            body: "",
            status: 201,
            content_type: Some("application/octet-stream"),
            item: Some("binary-1"),
            item_written: true,
        },
        Row {
            case: "a handler returning a document with no statusCode: 502, and the write it \
                   did do still stands",
            verb: "GET",
            path: "/things/t-4?mode=malformed&id=malformed-1".to_string(),
            keyed: false,
            body: "",
            status: 502,
            content_type: Some("application/json"),
            item: Some("malformed-1"),
            item_written: true,
        },
        Row {
            case: "a handler that raises: 502, with nothing written",
            verb: "GET",
            path: "/things/t-5?mode=raise&id=raised-1".to_string(),
            keyed: false,
            body: "",
            status: 502,
            content_type: Some("application/json"),
            item: Some("raised-1"),
            item_written: false,
        },
        Row {
            case: "an integration whose uri names the function by name alone",
            verb: "GET",
            path: "/named?id=by-name-1".to_string(),
            keyed: false,
            body: "",
            status: 200,
            content_type: Some("application/json"),
            item: Some("by-name-1"),
            item_written: true,
        },
        Row {
            case: "a path the function's policy grants no invocation for: 500, with the \
                   handler never reached",
            verb: "GET",
            path: "/ungranted?id=ungranted-1".to_string(),
            keyed: false,
            body: "",
            status: 500,
            content_type: Some("application/json"),
            item: Some("ungranted-1"),
            item_written: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
        if row.keyed {
            headers.push(("x-api-key", key_value.as_str()));
        }
        let answered = invoke(
            &router,
            &api,
            row.verb,
            &row.path,
            &headers,
            Bytes::from(row.body),
        )
        .await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?} ({} {}): expected HTTP {}, got {} with {:?}",
            row.verb,
            row.path,
            row.status,
            answered.status,
            answered.text()
        );
        if let Some(expected) = row.content_type {
            assert_eq!(
                answered.header("content-type").as_deref(),
                Some(expected),
                "case {case:?}: expected the media type {expected:?}, got {:?}",
                answered.header("content-type")
            );
        }
        if let Some(id) = row.item {
            let held = item_exists(&router, id).await;
            assert_eq!(
                held, row.item_written,
                "case {case:?}: expected the table to hold {id:?} = {}, got {held}",
                row.item_written
            );
        }
    }

    // The keyed control's body is the handler's own document, and the refused one carries
    // the gateway response's sentence rather than anything the function produced.
    let keyed = invoke(
        &router,
        &api,
        "GET",
        "/items?id=withkey-2",
        &[("x-api-key", key_value.as_str())],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        keyed.json(),
        json!({ "stored": "withkey-2" }),
        "case \"the keyed control's body\": expected the handler's own document, got {:?}",
        keyed.text()
    );
    let keyless = invoke(&router, &api, "GET", "/items?id=nokey-2", &[], Bytes::new()).await;
    assert_eq!(
        keyless.json(),
        json!({ "message": "Forbidden" }),
        "case \"the keyless refusal's body\": expected the gateway response's sentence, got \
         {:?}",
        keyless.text()
    );
    assert!(
        !item_exists(&router, "nokey-2").await,
        "case \"the keyless refusal's side channel\": expected no item for nokey-2"
    );

    // The event the function was handed: every part of the method request, as the ECHO row
    // reported it back.
    let echoed = invoke(
        &router,
        &api,
        "POST",
        "/things/t-9?mode=echo&tag=a&tag=b",
        &[
            ("content-type", "application/json"),
            ("x-api-key", key_value.as_str()),
            ("authorization", &authorization("execute-api")),
        ],
        Bytes::from_static(br#"{"sent":true}"#),
    )
    .await;
    let mut event = echoed.json();
    // The header names travel in their own member, asserted below: which headers the request
    // carried is not fixed by this suite (the router adds its own), so the whole-event
    // comparison is over the members that are.
    let header_names = event
        .as_object_mut()
        .and_then(|members| members.remove("headerNames"))
        .unwrap_or(Value::Null);
    let expected = json!({
        "httpMethod": "POST",
        "resource": "/things/{id}",
        "path": "/things/t-9",
        "pathParameters": { "id": "t-9" },
        // `tag` is sent twice: the single-valued map carries the LAST value, the
        // multi-valued one beside it carries them all.
        "query": { "mode": "echo", "tag": "b" },
        "multiQuery": { "mode": ["echo"], "tag": ["a", "b"] },
        "stageVariables": { STAGE_VARIABLE: STAGE_VARIABLE_VALUE },
        "requestBody": r#"{"sent":true}"#,
        "isBase64Encoded": false,
        "apiId": api.clone(),
        "stage": STAGE,
        "contextPath": format!("/{STAGE}/things/t-9"),
        "resourceId": one_thing.clone(),
    });
    assert_eq!(
        event, expected,
        "case \"the proxy event the function was handed\": expected {expected}, got {event}"
    );
    let names: Vec<String> = header_names
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.iter().any(|name| name == "x-api-key"),
        "case \"the headers the event carries\": expected the request's own headers, got \
         {names:?}"
    );
    assert!(
        names.iter().all(|name| name != "authorization"),
        "case \"the credential the event withholds\": expected no authorization header, got \
         {names:?}"
    );

    // A handler's repeated response header reaches the client as repeated field lines, which
    // is the only way one name carries every value it declared.
    let binary = invoke(
        &router,
        &api,
        "GET",
        "/things/t-8?mode=binary",
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        binary.body.as_ref(),
        [0u8, 1, 2].as_slice(),
        "case \"a base64 body\": expected the decoded bytes, got {:?}",
        binary.body
    );
    assert_eq!(
        binary.every_header("x-seen"),
        vec!["one".to_string(), "two".to_string()],
        "case \"a repeated response header\": expected both values, got {:?}",
        binary.every_header("x-seen")
    );
}
