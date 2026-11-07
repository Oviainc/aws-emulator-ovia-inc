// SPDX-License-Identifier: CC0-1.0

//! What a request SERVED by a deployed `AWS_PROXY` integration leaves behind in CloudWatch
//! Logs: the log of the run that answered it.
//!
//! The rule under test, one level above the recorded case:
//!
//! > A request a deployed stage answers by INVOKING its integration's Lambda function is one
//! > run of that function, so the function's own log group — `/aws/lambda/<function name>`
//! > for a function that names no other — holds that run's log afterwards: the runtime's
//! > `START`/`END`/`REPORT RequestId:` framing around every line the handler printed, in the
//! > stream that run reported as its own. One request served is one run logged, so N
//! > requests leave N runs' logs.
//!
//! Only a cross-service suite can pin this: the api, the stage and the integration are API
//! Gateway's, the run is AWS Lambda's, and the group the lines land in is CloudWatch Logs' —
//! all three on ONE engine, as `core/router/src/lib.rs` mounts them. Every assertion reads
//! the outcome back through CloudWatch Logs' own reader (`FilterLogEvents`), so a line that
//! did not reach that store cannot pass.
//!
//! The rows differ along what the rule generalises over: the VERB and the shape of the
//! request (a `POST` carrying a JSON body, a `GET` carrying a query string and no body) and
//! the REPETITION — the same client calling the same stage again, which is a second run and
//! therefore a second run's log beside the first.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to, and the resource path of its one method.
const STAGE: &str = "prod";
const RESOURCE_PATH: &str = "items";

/// The function every method of the api is integrated with, and the group its runs log to —
/// "By default, Lambda functions send logs to a default log group named
/// `/aws/lambda/<function name>`" (`LoggingConfig$LogGroup`).
const FUNCTION: &str = "proxy-logger";

/// The role the created function names. No IAM store is mounted here, so it is a value the
/// create carries rather than a record this suite relies on.
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";

/// The handler the deployment package declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for another service's clock-owed transition (a created function
/// becoming `Active`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The runtime records that frame one run.
const START_RECORD: &str = "START RequestId: ";
const REPORT_RECORD: &str = "REPORT RequestId: ";

/// What the handler prints before it answers, which is the line a client reading the
/// function's log group looks for.
const PRINTED_PREFIX: &str = "PROCESSED:";

/// The handler behind the method: it prints one line naming the item it was handed and
/// answers the proxy response envelope.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    sent = json.loads(event.get("body") or "{}")
    which = sent.get("id") or (event.get("queryStringParameters") or {}).get("id") or "none"
    print("PROCESSED:" + which)
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"seen": which}),
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
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
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
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

/// One CloudWatch Logs call over its `awsJson1_1` wire, answering its status and parsed body.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let answered = dispatch(
        router,
        "POST",
        &format!("logs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("authorization", &authorization("logs")),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
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
    body: &str,
) -> Answered {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        &[("content-type", "application/json")],
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

/// The messages `FilterLogEvents` reports for the function's own log group, each with the
/// stream it is in.
async fn delivered(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    let group = format!("/aws/lambda/{FUNCTION}");
    let (status, body) = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from FilterLogEvents on {group:?}, got {status} with \
         {body}"
    );
    body.get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    (
                        event
                            .get("logStreamName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        event
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The whole setup: a function, an api with one `POST`/`GET` method in front of it, the
/// grant that admits the invocation and a deployment of it to the stage. Answers the api id.
async fn deployed(router: &MultiServiceRouter) -> String {
    let (status, function) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
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

    let api = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "proxy-logging-api" })),
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
    let resource = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource = member("CreateResource", &resource, "id");
    for verb in ["POST", "GET"] {
        apigw(
            router,
            "PutMethod",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
        apigw(
            router,
            "PutIntegration",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {function_arn}/invocations"
                ),
            })),
            201,
        )
        .await;
    }

    // "Grants a principal permission to use a function" (`AddPermission`): the grant that
    // admits the invocation API Gateway makes as its own service principal.
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigateway-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": format!(
                "arn:aws:execute-api:{REGION}:{account}:{api}/*/*/{RESOURCE_PATH}",
                account = engine::emulator_account_id(),
            ),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission: expected HTTP 201, got {status} with {granted}"
    );

    apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    wait_until_active(router).await;
    api
}

/// One row: a request to the deployed stage, and the item its run reports having seen.
struct Row {
    case: &'static str,
    verb: &'static str,
    /// The path within the stage, query string included.
    path: &'static str,
    /// The request body, empty for a verb that carries none.
    body: &'static str,
    /// The value the handler names in its printed line and its answer.
    seen: &'static str,
}

/// The rows: the verbs and shapes of request a client sends the stage, the last one a repeat
/// of the first.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a POST carrying a JSON body",
            verb: "POST",
            path: "/items",
            body: r#"{"id": "first", "data": "one"}"#,
            seen: "first",
        },
        Row {
            case: "a GET carrying a query string and no body",
            verb: "GET",
            path: "/items?id=second",
            body: "",
            seen: "second",
        },
        Row {
            case: "a second POST from the same client",
            verb: "POST",
            path: "/items",
            body: r#"{"id": "third", "data": "three"}"#,
            seen: "third",
        },
    ]
}

/// Every request a deployed proxy integration serves leaves that run's log — the framing and
/// the handler's own printed line — in the function's log group, one stream per run.
#[tokio::test]
async fn a_served_proxy_request_leaves_its_runs_log_in_the_functions_group() {
    let router = emulator();
    let api = deployed(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let answered = invoke(&router, &api, row.verb, row.path, row.body).await;
        assert_eq!(
            (answered.status, answered.json()),
            (200, json!({ "seen": row.seen })),
            "case {case:?}: expected the handler's own 200 and body, got {} with {:?}",
            answered.status,
            answered.text()
        );

        // The group holds ONE run's log per request served, so the counts grow with the
        // requests: the framing records and the handler's line of each run.
        let events = delivered(&router, case).await;
        let messages: Vec<&str> = events.iter().map(|(_, message)| message.as_str()).collect();
        let served = index + 1;
        for (record, what) in [(START_RECORD, "START"), (REPORT_RECORD, "REPORT")] {
            let held = messages
                .iter()
                .filter(|message| message.starts_with(record))
                .count();
            assert_eq!(
                held, served,
                "case {case:?}: expected one {what} record per request served ({served}), got \
                 {held} in {messages:?}"
            );
        }
        let printed: Vec<&&str> = messages
            .iter()
            .filter(|message| message.starts_with(PRINTED_PREFIX))
            .collect();
        assert_eq!(
            printed.len(),
            served,
            "case {case:?}: expected one printed line per request served ({served}), got \
             {printed:?}"
        );
        // The handler's own line, as one event of its own: a log event carries the line it
        // was written as, without the break that ended it.
        let expected = format!("{PRINTED_PREFIX}{}", row.seen);
        assert!(
            messages.contains(&expected.as_str()),
            "case {case:?}: expected this run's own line {expected:?} among {messages:?}"
        );

        // One run writes one stream, so the group holds as many streams as runs: a repeat of
        // the same request is a second run and not a second entry in the first one's stream.
        let streams: Vec<&str> = {
            let mut held: Vec<&str> = events.iter().map(|(stream, _)| stream.as_str()).collect();
            held.sort_unstable();
            held.dedup();
            held
        };
        assert_eq!(
            streams.len(),
            served,
            "case {case:?}: expected one stream per request served ({served}), got {streams:?}"
        );
    }
}
