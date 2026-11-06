// SPDX-License-Identifier: CC0-1.0

//! A SAM-transformed function really writes the table the same stack declares: the stack is
//! deployed in CloudFormation, the function is invoked through AWS Lambda, and the item it
//! wrote is read back out of Amazon DynamoDB through DynamoDB's own API.
//!
//! The rule under test: **AWS Lambda's managed `python3.x` runtimes ship the AWS SDK for
//! Python on the function's import path, so `import boto3` at module scope succeeds and the
//! handler calls another AWS service with no dependency of its own — the invocation answers
//! HTTP 200 with NO `X-Amz-Function-Error` and the handler's own JSON result as the payload,
//! and the work the call reported is work the called service really did.** Four crates have
//! to agree for that to hold (CloudFormation expands the macro and makes the calls, Lambda
//! holds and runs the function, DynamoDB serves the table the handler writes, IAM answers
//! for the role the transform synthesized), so the suite lives here rather than in any one
//! of them, and it drives the emulator's own assembled service set.
//!
//! The rows differ along what the rule generalises over: WHICH `python3.x` runtime the
//! function declares, the module and attribute its `Handler` names (and so the file the
//! inline package is archived as), the form the SDK is imported in (`import boto3`,
//! `from boto3 import client`), WHICH DynamoDB operation the handler calls (a single write,
//! a write then a read in the same invocation, a batch write of three items, an update
//! expression over an item it just put, a write the service refuses), how many items the
//! invocation leaves in the table, and the language the template is written in.
//!
//! Each row asserts both halves of the rule — what the invocation answered, which is what
//! the handler returned, and what the TABLE holds afterwards, read through `GetItem` — so a
//! call that answered without doing the work, or did the work without answering, fails here.
//! Every expected value is computed here from what the row's event carries, never read back
//! out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, physical_ids_of};

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation of these handlers is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the SDK the runtime supplies and not
/// how fast an interpreter gets through it.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of the function's state, and how many polls to make.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The emulator as it is ASSEMBLED, on one engine (`aws_emulator_router::build_router`): the
/// handler's SDK call is dispatched back into this same router, so the table it writes is
/// the table this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// Call one DynamoDB operation over its `awsJson1_0` wire.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("authorization", authorization("dynamodb"))
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, and
/// answer the configuration it reported then.
///
/// The span a function spends `Pending` is that service's own, so no fixed sleep names the
/// instant it ends.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..POLLS {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &[],
        )
        .await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        last = configuration;
        tokio::time::sleep(POLL).await;
    }
    last
}

/// Poll `DescribeTable` until DynamoDB first reports the table `ACTIVE`, and answer the
/// status it reported then.
///
/// A created table is reported `CREATING` until that service's own clock-owed transition
/// falls due — "you can perform read and write operations only on an ACTIVE table" — so the
/// handler is invoked once the table it writes is one DynamoDB serves writes on. The span is
/// DynamoDB's own, so nothing here sleeps a fixed span on its behalf.
async fn wait_until_table_active(router: &MultiServiceRouter, table: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLLS {
        let (_, described) = dynamodb(router, "DescribeTable", &json!({ "TableName": table })).await;
        let status = described
            .get("Table")
            .and_then(|table| table.get("TableStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == "ACTIVE" {
            return status;
        }
        last = status;
        tokio::time::sleep(POLL).await;
    }
    last
}

/// What the TABLE must hold after one invocation: one entry per key read back, with the
/// item `GetItem` must report for it, or `None` for a key no item was written under.
type HeldItems = Vec<(String, Option<Value>)>;

/// One row of the table: the function it deploys, and what must come of invoking it.
struct Row {
    case: &'static str,
    /// The `python3.x` runtime the function declares.
    runtime: &'static str,
    /// The `Handler` it declares: the module the inline package is archived as, and the
    /// attribute in it the runtime calls.
    handler: &'static str,
    /// Whether the template is written in YAML rather than JSON.
    yaml: bool,
    /// The handler's own source, as the declaration's `InlineCode`.
    code: &'static str,
    /// What the handler must return for an event carrying `(pk, body)`, which is the payload
    /// the invocation must answer.
    expected_payload: fn(&str, &str) -> Value,
    /// What the TABLE must hold afterwards ([`HeldItems`]), computed from the event.
    expected_items: fn(&str, &str) -> HeldItems,
}

/// A handler that writes the item the event names, with the SDK client built once at module
/// scope — the idiom a real function uses, and the one that fails outright when the runtime
/// ships no SDK.
const PUT_AT_MODULE_SCOPE: &str = r#"
import os

import boto3

TABLE = os.environ["TABLE"]
CLIENT = boto3.client("dynamodb")


def handler(event, context):
    CLIENT.put_item(
        TableName=TABLE,
        Item={"pk": {"S": event["pk"]}, "body": {"S": event["body"]}},
    )
    return {"written": event["pk"]}
"#;

/// A handler that reads its own write back in the same invocation, so the invocation's
/// payload carries what DynamoDB answered the second call with.
const PUT_THEN_GET: &str = r#"
import os

import boto3


def lambda_handler(event, context):
    table = os.environ["TABLE"]
    client = boto3.client("dynamodb")
    client.put_item(
        TableName=table,
        Item={"pk": {"S": event["pk"]}, "body": {"S": event["body"]}},
    )
    read = client.get_item(TableName=table, Key={"pk": {"S": event["pk"]}})
    return {"read": read["Item"]["body"]["S"], "attributes": len(read["Item"])}
"#;

/// A handler importing the client factory by name, writing three items in one batch.
const BATCH_WRITE: &str = r#"
import os

from boto3 import client


def main(event, context):
    table = os.environ["TABLE"]
    ddb = client("dynamodb")
    requests = [
        {
            "PutRequest": {
                "Item": {
                    "pk": {"S": "%s-%d" % (event["pk"], index)},
                    "body": {"N": str(index)},
                }
            }
        }
        for index in range(3)
    ]
    answered = ddb.batch_write_item(RequestItems={table: requests})
    return {"unprocessed": len(answered.get("UnprocessedItems") or {})}
"#;

/// A handler that puts an item and then updates it with an update expression, reporting what
/// DynamoDB answered with for `ALL_NEW`.
const PUT_THEN_UPDATE: &str = r##"
import os

import boto3


def handler(event, context):
    table = os.environ["TABLE"]
    client = boto3.client("dynamodb")
    client.put_item(TableName=table, Item={"pk": {"S": event["pk"]}})
    answered = client.update_item(
        TableName=table,
        Key={"pk": {"S": event["pk"]}},
        UpdateExpression="SET #b = :b",
        ExpressionAttributeNames={"#b": "body"},
        ExpressionAttributeValues={":b": {"S": event["body"]}},
        ReturnValues="ALL_NEW",
    )
    return {"body": answered["Attributes"]["body"]["S"]}
"##;

/// A handler whose call the service REFUSES: the refusal is DynamoDB's own, raised as the
/// SDK's `ClientError`, and the handler still runs to completion.
const REFUSED_WRITE: &str = r#"
import os

import boto3
import botocore.exceptions


def handler(event, context):
    client = boto3.client("dynamodb")
    try:
        client.put_item(
            TableName=os.environ["TABLE"] + "-absent",
            Item={"pk": {"S": event["pk"]}},
        )
    except botocore.exceptions.ClientError as error:
        return {"code": error.response["Error"]["Code"]}
    return {"code": "no refusal at all"}
"#;

const ROWS: &[Row] = &[
    Row {
        case: "import boto3 at module scope: put_item writes the item the event names",
        runtime: "python3.12",
        handler: "index.handler",
        yaml: false,
        code: PUT_AT_MODULE_SCOPE,
        expected_payload: |pk, _body| json!({ "written": pk }),
        expected_items: |pk, body| {
            vec![(
                pk.to_string(),
                Some(json!({ "pk": { "S": pk }, "body": { "S": body } })),
            )]
        },
    },
    Row {
        case: "a handler on another module and attribute reads its own write back through \
               get_item in the same invocation",
        runtime: "python3.13",
        handler: "app.lambda_handler",
        yaml: false,
        code: PUT_THEN_GET,
        expected_payload: |_pk, body| json!({ "read": body, "attributes": 2 }),
        expected_items: |pk, body| {
            vec![(
                pk.to_string(),
                Some(json!({ "pk": { "S": pk }, "body": { "S": body } })),
            )]
        },
    },
    Row {
        case: "from boto3 import client: batch_write_item writes three items in one call",
        runtime: "python3.9",
        handler: "writer.main",
        yaml: false,
        code: BATCH_WRITE,
        expected_payload: |_pk, _body| json!({ "unprocessed": 0 }),
        expected_items: |pk, _body| {
            (0..3)
                .map(|index| {
                    let key = format!("{pk}-{index}");
                    let item =
                        json!({ "pk": { "S": key }, "body": { "N": index.to_string() } });
                    (key, Some(item))
                })
                .collect()
        },
    },
    Row {
        case: "the YAML spelling of the template, with an update expression over the item the \
               handler just put",
        runtime: "python3.11",
        handler: "index.handler",
        yaml: true,
        code: PUT_THEN_UPDATE,
        expected_payload: |_pk, body| json!({ "body": body }),
        expected_items: |pk, body| {
            vec![(
                pk.to_string(),
                Some(json!({ "pk": { "S": pk }, "body": { "S": body } })),
            )]
        },
    },
    Row {
        case: "a call the service refuses reaches the handler as DynamoDB's own error code, \
               and nothing is written",
        runtime: "python3.12",
        handler: "index.handler",
        yaml: false,
        code: REFUSED_WRITE,
        // "The operation tried to access a nonexistent table" is DynamoDB's
        // `ResourceNotFoundException` (`smithy_query(operation="PutItem")`).
        expected_payload: |_pk, _body| json!({ "code": "ResourceNotFoundException" }),
        expected_items: |pk, _body| vec![(pk.to_string(), None)],
    },
];

/// The SAM template a row deploys: an `AWS::Serverless::Function` whose inline code is the
/// row's handler, and the `AWS::Serverless::SimpleTable` it writes, named to the function
/// through the environment variable the transform carries through.
fn template(row: &Row) -> String {
    if !row.yaml {
        return json!({
            "Transform": "AWS::Serverless-2016-10-31",
            "Resources": {
                "Writer": {
                    "Type": "AWS::Serverless::Function",
                    "Properties": {
                        "Runtime": row.runtime,
                        "Handler": row.handler,
                        "Timeout": FUNCTION_TIMEOUT,
                        "InlineCode": row.code,
                        "Environment": { "Variables": { "TABLE": { "Ref": "Items" } } },
                        "Policies": [
                            { "DynamoDBCrudPolicy": { "TableName": { "Ref": "Items" } } }
                        ]
                    }
                },
                "Items": {
                    "Type": "AWS::Serverless::SimpleTable",
                    "Properties": { "PrimaryKey": { "Name": "pk", "Type": "String" } }
                }
            }
        })
        .to_string();
    }
    // The same declaration in YAML, with the handler's source as a block scalar: the rule
    // is about the function the transform emits, which is the same function either way.
    let indented: Vec<String> = row
        .code
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("        {line}")
            }
        })
        .collect();
    let mut yaml = vec![
        "Transform: AWS::Serverless-2016-10-31".to_string(),
        "Resources:".to_string(),
        "  Items:".to_string(),
        "    Type: AWS::Serverless::SimpleTable".to_string(),
        "    Properties:".to_string(),
        "      PrimaryKey:".to_string(),
        "        Name: pk".to_string(),
        "        Type: String".to_string(),
        "  Writer:".to_string(),
        "    Type: AWS::Serverless::Function".to_string(),
        "    Properties:".to_string(),
        format!("      Runtime: {}", row.runtime),
        format!("      Handler: {}", row.handler),
        format!("      Timeout: {FUNCTION_TIMEOUT}"),
        "      Environment:".to_string(),
        "        Variables:".to_string(),
        "          TABLE: !Ref Items".to_string(),
        "      Policies:".to_string(),
        "        - DynamoDBCrudPolicy:".to_string(),
        "            TableName: !Ref Items".to_string(),
        "      InlineCode: |".to_string(),
    ];
    yaml.extend(indented);
    yaml.push(String::new());
    yaml.join("\n")
}

/// Deploy one row's stack and answer the physical ids its resources were created under.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    row: &Row,
) -> BTreeMap<String, String> {
    let body = template(row);
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            // A template declaring a macro is acknowledged with CAPABILITY_AUTO_EXPAND, and
            // the role the transform synthesizes for the function with CAPABILITY_IAM.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );

    let (reached, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    let failures = if reached == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) =
            cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
         resource failures {failures:?}"
    );

    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    let physical = physical_ids_of(&resources);
    for logical in ["Writer", "Items"] {
        assert!(
            physical.contains_key(logical),
            "case {case:?}: expected a physical id for {logical}, got {resources}"
        );
    }
    physical
}

/// The item DynamoDB holds under `pk`, read through its own `GetItem`.
async fn item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    pk: &str,
) -> Option<Value> {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { "pk": { "S": pk } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetItem for {pk:?}, got {status} with body \
         {answered}"
    );
    answered.get("Item").cloned()
}

/// One deploy, one invocation and one read-back per row.
#[tokio::test]
async fn a_sam_transformed_handler_writes_its_table_through_the_sdk_the_runtime_supplies() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = build_router();
        let stack = format!("sam-writer-{index}");
        let physical = deploy(&router, case, &stack, row).await;
        let function = physical["Writer"].clone();
        let table = physical["Items"].clone();

        let configuration = wait_until_active(&router, &function).await;
        assert_eq!(
            configuration.get("State").and_then(Value::as_str),
            Some("Active"),
            "case {case:?}: expected Lambda to report {function} Active, got {configuration}"
        );
        let table_status = wait_until_table_active(&router, &table).await;
        assert_eq!(
            table_status, "ACTIVE",
            "case {case:?}: expected DynamoDB to report {table} ACTIVE, got {table_status:?}"
        );

        let pk = format!("item-{index}");
        let body = format!("written-by-row-{index}");
        let event = json!({ "pk": pk, "body": body });
        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            event.to_string().as_bytes(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} with payload {payload}"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            signal, None,
            "case {case:?}: expected no function error, got {signal:?} with payload {payload}"
        );
        let expected = (row.expected_payload)(&pk, &body);
        assert_eq!(
            payload, expected,
            "case {case:?}: expected the handler's own result {expected}, got {payload}"
        );

        // What the TABLE holds: the work the handler reported is work DynamoDB really did.
        for (key, wanted) in (row.expected_items)(&pk, &body) {
            let held = item(&router, case, &table, &key).await;
            assert_eq!(
                held, wanted,
                "case {case:?}: expected the table to hold {wanted:?} under {key:?}, got \
                 {held:?}"
            );
        }
    }
}
