// SPDX-License-Identifier: CC0-1.0

//! The whole HTTP -> API Gateway -> AWS Lambda -> Amazon DynamoDB ingest path of a deployed
//! REST stage, and what the invocation that path makes leaves in Amazon CloudWatch Logs.
//!
//! The rule under test: **an invocation of a Lambda function leaves that run's LOG in the
//! function's own CloudWatch Logs group — the group a read of the function reports in its
//! `LoggingConfig$logGroup`, created on the first invocation that writes to it, in the stream
//! the run itself names — and the records in it are the runtime's own framing lines around
//! every line the handler printed. The run is the same whoever made it, so an invocation a
//! deployed `AWS_PROXY` stage made leaves exactly what a direct `Invoke` does.** Five crates
//! have to agree for that to hold (CloudFormation expands the macro and provisions, API
//! Gateway resolves the stage and performs the proxy integration, Lambda runs the function,
//! DynamoDB serves the table the handler writes, CloudWatch Logs holds the group), so the
//! suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: WHO invoked the function (a client of
//! the deployed stage, and Lambda's own `Invoke`) and how many lines the handler printed (one
//! and three).
//!
//! Each row also asserts the two halves of the pipeline the stage drives — the status and body
//! the client was served, which is what the handler returned, and the item DynamoDB holds
//! afterwards, read through its own strongly consistent `GetItem` — so a call that answered
//! without doing the work, or did the work without answering, fails here.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, physical_ids_of};

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The `StackStatus` the setup waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The stage the api is deployed to, and the resource path of its one method.
const STAGE: &str = "prod";
const RESOURCE_PATH: &str = "items";

/// How long one invocation of this handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// The fixed framing of the record the Lambda runtime writes for every run it finishes.
const REPORT_PREFIX: &str = "REPORT RequestId: ";

/// The handler the stack deploys: it writes the posted item, prints one line per `lines` the
/// payload asks for, and answers the proxy envelope the client is served.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3


def handler(event, context):
    posted = json.loads(event.get("body") or "{}")
    boto3.client("dynamodb").put_item(
        TableName=os.environ["TABLE"],
        Item={"id": {"S": posted["id"]}, "data": {"S": posted["data"]}},
    )
    for index in range(int(posted.get("lines", 1))):
        print("handled %s line %d" % (posted["id"], index))
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"stored": posted["id"]}),
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator as it is ASSEMBLED, on one engine: the handler's SDK call is dispatched back
/// into this same router, so the table it writes and the group its run logs to are the ones
/// this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: a control-plane call carries a credential
    // scope so the router can name the target service, and the invoke of the published stage
    // URL carries none, as a client of one sends none.
    router.set_request_authentication(false);
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

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "POST",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("cloudformation")),
        ],
        Bytes::from(body),
    )
    .await
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
        answered.status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
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

/// One Amazon DynamoDB call over its `awsJson1_0` wire.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> Answered {
    dispatch(
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
    .await
}

/// One Amazon CloudWatch Logs call over its `awsJson1_1` wire.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> Answered {
    dispatch(
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
    .await
}

/// A required string member of a JSON object.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, and
/// answer the configuration it reported then. The span is that service's own, so nothing here
/// sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        let configuration = reported.json();
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// Poll `DescribeTable` until DynamoDB first reports the table `ACTIVE` — "you can perform
/// read and write operations only on an ACTIVE table".
async fn wait_until_table_active(router: &MultiServiceRouter, table: &str) {
    for _ in 0..POLLS {
        let described = dynamodb(router, "DescribeTable", json!({ "TableName": table })).await;
        if described
            .json()
            .get("Table")
            .and_then(|table| table.get("TableStatus"))
            .and_then(Value::as_str)
            == Some("ACTIVE")
        {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: DynamoDB never reported {table} ACTIVE");
}

/// Every message CloudWatch Logs holds in `group`, polled until the group holds at least
/// `at_least` of them or the polls run out.
async fn messages_in(router: &MultiServiceRouter, group: &str, at_least: usize) -> Vec<String> {
    let mut held: Vec<String> = Vec::new();
    for _ in 0..POLLS {
        let answered = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
        if answered.status == 200 {
            held = answered
                .json()
                .get("events")
                .and_then(Value::as_array)
                .map(|events| {
                    events
                        .iter()
                        .filter_map(|event| event.get("message").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if held.len() >= at_least {
                return held;
            }
        }
        tokio::time::sleep(POLL).await;
    }
    held
}

/// The SAM template the suite deploys: the handler above, and the table it writes, named to
/// the function through the environment variable the transform carries through.
fn template() -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Writer": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": FUNCTION_TIMEOUT,
                    "InlineCode": HANDLER_SOURCE,
                    "Environment": { "Variables": { "TABLE": { "Ref": "Items" } } },
                    "Policies": [
                        { "DynamoDBCrudPolicy": { "TableName": { "Ref": "Items" } } }
                    ]
                }
            },
            "Items": {
                "Type": "AWS::Serverless::SimpleTable",
                "Properties": { "PrimaryKey": { "Name": "id", "Type": "String" } }
            }
        }
    })
    .to_string()
}

/// How the function under a row is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// A client of the deployed stage's own invoke URL, through the `AWS_PROXY` integration.
    DeployedStage,
    /// Lambda's own `Invoke`, handed the same event body the stage would have built.
    DirectInvoke,
}

/// One row: who invokes the function, and how many lines its handler prints.
struct Row {
    case: &'static str,
    caller: Caller,
    lines: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "a client of the deployed stage, whose handler prints three lines",
        caller: Caller::DeployedStage,
        lines: 3,
    },
    Row {
        case: "Lambda's own Invoke of the same handler, printing one line",
        caller: Caller::DirectInvoke,
        lines: 1,
    },
];

#[tokio::test]
async fn a_deployed_proxy_stage_serves_the_post_and_its_invocation_logs_to_the_functions_group() {
    // ONE emulator for every row: each row deploys a stack of its own, so the function, the
    // table and the group a row's run writes to are still that row's alone.
    let router = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;

        // 1. The stack: the function and the table it writes.
        let stack = format!("proxy-pipeline-{index}");
        let body = template();
        let created = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &body),
                // A template declaring a macro is acknowledged with CAPABILITY_AUTO_EXPAND,
                // and the role the transform synthesizes for the function with CAPABILITY_IAM.
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "setup {case:?}: expected HTTP 200 from CreateStack, got {} with {:?}",
            created.status,
            created.text()
        );
        let (reached, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            reached, CREATE_COMPLETE,
            "setup {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );
        let resources =
            cloudformation(&router, "DescribeStackResources", &[("StackName", &stack)]).await;
        let physical = physical_ids_of(&resources.text());
        let function_name = physical
            .get("Writer")
            .cloned()
            .unwrap_or_else(|| panic!("setup {case:?}: no physical id for Writer"));
        let table = physical
            .get("Items")
            .cloned()
            .unwrap_or_else(|| panic!("setup {case:?}: no physical id for Items"));

        let configuration = wait_until_active(&router, &function_name).await;
        wait_until_table_active(&router, &table).await;
        // The group the function itself reports is the one its run has to write to.
        let expected_group = configuration
            .get("LoggingConfig")
            .and_then(|logging| logging.get("LogGroup"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            expected_group,
            format!("/aws/lambda/{function_name}"),
            "setup {case:?}: expected the function's default log group, got \
             {expected_group:?}"
        );
        let function_arn = member("GetFunctionConfiguration", &configuration, "FunctionArn");

        // The grant that admits an invocation from API Gateway.
        let granted = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function_name}/policy"),
            Some(json!({
                "StatementId": "apigw-invoke",
                "Action": "lambda:InvokeFunction",
                "Principal": "apigateway.amazonaws.com",
            })),
        )
        .await;
        assert_eq!(
            granted.status, 201,
            "setup {case:?}: expected HTTP 201 from AddPermission, got {} with {:?}",
            granted.status,
            granted.text()
        );

        // 2. The api: POST /items behind an AWS_PROXY integration on that function, deployed.
        let api_record = apigw(
            &router,
            "CreateRestApi",
            "POST",
            "/restapis",
            Some(json!({ "name": format!("pipeline-api-{index}") })),
            201,
        )
        .await;
        let api = member("CreateRestApi", &api_record, "id");
        let root = member("CreateRestApi", &api_record, "rootResourceId");
        let resource = apigw(
            &router,
            "CreateResource",
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": RESOURCE_PATH })),
            201,
        )
        .await;
        let resource_id = member("CreateResource", &resource, "id");
        let method = format!("/restapis/{api}/resources/{resource_id}/methods/POST");
        apigw(
            &router,
            "PutMethod",
            "PUT",
            &method,
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
        apigw(
            &router,
            "PutIntegration",
            "PUT",
            &format!("{method}/integration"),
            Some(json!({
                "type": "AWS_PROXY",
                // `integrationHttpMethod` travels under its `jsonName`, which is what
                // `smithy_query(query="operation",operation="PutIntegration")` reports.
                "httpMethod": "POST",
                "uri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {function_arn}/invocations"
                ),
            })),
            201,
        )
        .await;
        apigw(
            &router,
            "CreateDeployment",
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
            201,
        )
        .await;

        // 3. The call: what the client is served, or what `Invoke` answered.
        let id = format!("posted-{index}");
        let data = format!("payload-{index}");
        let posted = json!({ "id": id, "data": data, "lines": row.lines });
        let served = match row.caller {
            Caller::DeployedStage => {
                dispatch(
                    &router,
                    "POST",
                    &format!("{api}.execute-api.{REGION}.amazonaws.com"),
                    &format!("/{STAGE}/{RESOURCE_PATH}"),
                    &[("content-type", "application/json")],
                    Bytes::from(posted.to_string()),
                )
                .await
            }
            Caller::DirectInvoke => {
                lambda(
                    &router,
                    "POST",
                    &format!("/2015-03-31/functions/{function_name}/invocations"),
                    Some(json!({ "body": posted.to_string() })),
                )
                .await
            }
        };
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected HTTP 200 from the {:?} call, got {} with {:?}",
            row.caller,
            served.status,
            served.text()
        );
        let answered = served.json();
        let reported: Value = match row.caller {
            // The stage serves the envelope's own `body`, which is the handler's JSON text.
            Caller::DeployedStage => answered.clone(),
            // `Invoke` answers the whole envelope the handler returned.
            Caller::DirectInvoke => answered
                .get("body")
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null),
        };
        assert_eq!(
            reported.get("stored").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: expected the handler's own result for {id:?}, got {answered}"
        );

        // 4. The work the call reported is work DynamoDB really did.
        let read = dynamodb(
            &router,
            "GetItem",
            json!({
                "TableName": table,
                "Key": { "id": { "S": id } },
                "ConsistentRead": true,
            }),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetItem, got {} with {:?}",
            read.status,
            read.text()
        );
        assert_eq!(
            read.json().get("Item").cloned().unwrap_or(Value::Null),
            json!({ "id": { "S": id }, "data": { "S": data } }),
            "case {case:?}: expected the table to hold the posted item under {id:?}, got {}",
            read.text()
        );

        // 5. What the run left in the function's own log group: every line the handler
        // printed, and the runtime's REPORT record for that run.
        let wanted: Vec<String> = (0..row.lines)
            .map(|line| format!("handled {id} line {line}"))
            .collect();
        let held = messages_in(&router, &expected_group, wanted.len() + 1).await;
        for line in &wanted {
            assert!(
                held.iter().any(|message| message.contains(line.as_str())),
                "case {case:?}: expected the group {expected_group:?} to hold the printed line \
                 {line:?}, got {held:?}"
            );
        }
        assert!(
            held.iter().any(|message| message.contains(REPORT_PREFIX)),
            "case {case:?}: expected the group {expected_group:?} to hold a {REPORT_PREFIX:?} \
             record, got {held:?}"
        );
    }
}
