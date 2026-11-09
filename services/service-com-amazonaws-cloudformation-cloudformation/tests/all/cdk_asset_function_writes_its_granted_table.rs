// SPDX-License-Identifier: CC0-1.0

//! The function a CDK-shaped deploy creates really RUNS and really writes the table the same
//! stack granted it: the stack is deployed the way `cdk deploy` deploys one (a `CREATE` change
//! set over the synthesized template, then `ExecuteChangeSet`), the function's deployment
//! package is the .zip asset a client staged in the bootstrap bucket, the function is invoked
//! through AWS Lambda, and the item its handler wrote is read back out of Amazon DynamoDB
//! through DynamoDB's own API.
//!
//! RULE under test: **a function AWS Lambda holds under a managed `python3.x` runtime runs
//! with that runtime's AWS SDK for Python on the handler's import path — whichever route the
//! deployment package took to Lambda, a template's own inline source or a .zip object staged
//! in Amazon S3 — so a module-scope `import boto3` succeeds, a synchronous `Invoke` answers
//! HTTP 200 with NO `X-Amz-Function-Error` header and the handler's own result as the payload,
//! and the call the handler reported making was really served by the other service.** Lambda
//! models no error for a missing SDK in the runtime: `FunctionError` is the
//! `X-Amz-Function-Error` header and is set only when the handler itself faulted
//! (`smithy_query(operation="Invoke")`).
//!
//! The rows differ along what the rule generalises over: HOW the deployment package reached
//! Lambda (an Amazon S3 asset object, as the CDK uploads one, and the template's own inline
//! source), WHICH `python3.x` runtime the function declares, the module and attribute its
//! `Handler` names (and so the file the package is archived under), the form the SDK is
//! imported in (`import boto3` at module scope, `from boto3 import client`,
//! `boto3.resource("dynamodb")`), WHICH DynamoDB operation the handler calls, and which grant
//! the template attaches to the function's role (the CDK's write-only grant, and its
//! read-and-write one).
//!
//! Each row asserts both halves of the rule — the payload the invocation answered, which is
//! what the handler returned, and what the TABLE holds afterwards, read through `GetItem` — so
//! an invocation that answered without doing the work, or a runtime that cannot import the SDK
//! at all, fails here. Every expected value is computed from what the row's event carries,
//! never read back out of the answer being checked.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{create_change_set, execute_change_set};
use crate::dynamic_references::put_parameter;
use crate::dynamodb_table_resources::{describe_table, dynamodb, failure_reasons};
use crate::hotswapped_function_code::{lambda, package};
use crate::stack_lifecycle::{
    acknowledging_iam, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, ACCOUNT_ID, REGION,
};
use crate::stack_resource_provisioning::s3;

/// The logical ids the CDK gives the constructs of the app this suite is written for: a
/// `ddb.Table`, a `_lambda.Function` and the role and inline policy its grant produces, plus
/// the metadata resource the CDK adds to every synthesized stack.
const TABLE_LOGICAL_ID: &str = "TableCD117FA1";
const ROLE_LOGICAL_ID: &str = "WriterServiceRole7D7F0988";
const POLICY_LOGICAL_ID: &str = "WriterServiceRoleDefaultPolicyA2BD6A50";
const FUNCTION_LOGICAL_ID: &str = "Writer9270CBC0";
const METADATA_LOGICAL_ID: &str = "CDKMetadata";

/// The name of the condition the CDK guards its metadata resource with.
const METADATA_CONDITION: &str = "CDKMetadataAvailable";

/// A property value of the form the CDK encodes its version report in.
const ANALYTICS: &str = "v2:deflate64:H4sIAAAAAAAA";

/// The Parameter Store parameter a `DefaultStackSynthesizer` stack reads the bootstrap version
/// from, and a version its own rule admits.
const BOOTSTRAP_VERSION_PARAMETER: &str = "/cdk-bootstrap/hnb659fds/version";
const BOOTSTRAP_VERSION: &str = "21";

/// The bucket a CDK app's file assets are staged in, as `cdk bootstrap` names it:
/// `cdk-<qualifier>-assets-<account>-<region>`.
fn assets_bucket() -> String {
    format!("cdk-hnb659fds-assets-{ACCOUNT_ID}-{REGION}")
}

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation of these handlers is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the SDK the runtime supplies and not
/// how fast an interpreter gets through a handler.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long between two readings of a resource that is settling, and how many readings before
/// the row is a failure. The spans are the owning services' own, so nothing here sleeps a
/// fixed span on their behalf.
const LOOK_AGAIN: Duration = Duration::from_millis(25);
const LOOKS: usize = 400;

/// The partition key every table below is declared with, and the event member the handlers
/// read it from.
const PARTITION_KEY: &str = "pk";

/// The attribute the handlers write beside the key, and the event member it comes from.
const MARKER_ATTRIBUTE: &str = "marker";

/// The actions the CDK's `table.grant_write_data(fn)` attaches to a function's role, as the
/// synthesized inline policy carries them.
const GRANT_WRITE_DATA: &[&str] = &[
    "dynamodb:BatchWriteItem",
    "dynamodb:PutItem",
    "dynamodb:UpdateItem",
    "dynamodb:DeleteItem",
    "dynamodb:DescribeTable",
];

/// The actions the CDK's `table.grant_read_write_data(fn)` attaches, for a handler that also
/// READS through the SDK in the same invocation.
const GRANT_READ_WRITE_DATA: &[&str] = &[
    "dynamodb:BatchGetItem",
    "dynamodb:GetItem",
    "dynamodb:Query",
    "dynamodb:Scan",
    "dynamodb:ConditionCheckItem",
    "dynamodb:BatchWriteItem",
    "dynamodb:PutItem",
    "dynamodb:UpdateItem",
    "dynamodb:DeleteItem",
    "dynamodb:DescribeTable",
];

/// A handler whose SDK client is built at MODULE scope — the idiom a CDK default-props
/// function carries, and the one that fails outright when the runtime ships no SDK.
const PUT_AT_MODULE_SCOPE: &str = r#"
import os

import boto3

TABLE = os.environ["TABLE"]
CLIENT = boto3.client("dynamodb")


def handler(event, context):
    CLIENT.put_item(
        TableName=TABLE,
        Item={"pk": {"S": event["pk"]}, "marker": {"S": event["marker"]}},
    )
    return {"written": event["pk"]}
"#;

/// A handler importing the client factory by name, which puts an item and then updates it with
/// an update expression, reporting what DynamoDB answered for `ALL_NEW`.
const PUT_THEN_UPDATE: &str = r##"
import os

from boto3 import client


def lambda_handler(event, context):
    table = os.environ["TABLE"]
    ddb = client("dynamodb")
    ddb.put_item(TableName=table, Item={"pk": {"S": event["pk"]}})
    answered = ddb.update_item(
        TableName=table,
        Key={"pk": {"S": event["pk"]}},
        UpdateExpression="SET #m = :m",
        ExpressionAttributeNames={"#m": "marker"},
        ExpressionAttributeValues={":m": {"S": event["marker"]}},
        ReturnValues="ALL_NEW",
    )
    return {"marker": answered["Attributes"]["marker"]["S"]}
"##;

/// A handler reaching the table through the SDK's RESOURCE interface, whose items are plain
/// Python values, and reading its own write back in the same invocation.
const RESOURCE_PUT_THEN_GET: &str = r#"
import os

import boto3


def main(event, context):
    table = boto3.resource("dynamodb").Table(os.environ["TABLE"])
    table.put_item(Item={"pk": event["pk"], "marker": event["marker"]})
    read = table.get_item(Key={"pk": event["pk"]})
    return {"read": read["Item"]["marker"], "attributes": len(read["Item"])}
"#;

/// A handler writing two items in ONE batch call, reporting what the service left
/// unprocessed.
const BATCH_WRITE: &str = r#"
import os

import boto3


def handler(event, context):
    ddb = boto3.client("dynamodb")
    requests = [
        {
            "PutRequest": {
                "Item": {
                    "pk": {"S": "%s-%d" % (event["pk"], index)},
                    "marker": {"N": str(index)},
                }
            }
        }
        for index in range(2)
    ]
    answered = ddb.batch_write_item(RequestItems={os.environ["TABLE"]: requests})
    return {"unprocessed": len(answered.get("UnprocessedItems") or {})}
"#;

/// How the deployment package reached AWS Lambda.
#[derive(Clone, Copy, Debug)]
enum Package {
    /// A .zip object staged in the CDK assets bucket and named by `Code.S3Bucket`/`Code.S3Key`,
    /// which is what `cdk deploy` uploads and declares.
    S3Asset,
    /// The template's own `Code.ZipFile` source text.
    InlineSource,
}

/// What the TABLE must hold after one invocation: one entry per key read back, with the item
/// `GetItem` must report for it, or `None` for a key no item was written under.
type HeldItems = Vec<(String, Option<Value>)>;

/// One row: the function one CDK-shaped stack deploys, and what invoking it owes.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// How the package reached Lambda.
    package: Package,
    /// The `python3.x` runtime the function declares.
    runtime: &'static str,
    /// The `Handler` it declares: the module the package carries the source as, and the
    /// attribute in it the runtime calls.
    handler: &'static str,
    /// The handler's own source.
    code: &'static str,
    /// The actions the synthesized inline policy grants the function's role.
    grant: &'static [&'static str],
    /// What the handler must return for an event carrying `(pk, marker)`.
    expected_payload: fn(&str, &str) -> Value,
    /// What the TABLE must hold afterwards, computed from the event.
    expected_items: fn(&str, &str) -> HeldItems,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The shape the recorded `cdk deploy` synthesizes: an asset package, default
            // props, and a write-only grant over the table.
            case: "an S3 asset package whose handler imports the SDK at module scope and \
                   writes the granted table",
            stack: "cdk-asset-writer",
            package: Package::S3Asset,
            runtime: "python3.12",
            handler: "index.handler",
            code: PUT_AT_MODULE_SCOPE,
            grant: GRANT_WRITE_DATA,
            expected_payload: |pk, _marker| json!({ "written": pk }),
            expected_items: |pk, marker| {
                vec![(
                    pk.to_string(),
                    Some(json!({ "pk": { "S": pk }, "marker": { "S": marker } })),
                )]
            },
        },
        Row {
            case: "an S3 asset package on another module and runtime, importing the client \
                   factory by name and updating the item it just put",
            stack: "cdk-asset-updater",
            package: Package::S3Asset,
            runtime: "python3.11",
            handler: "app.lambda_handler",
            code: PUT_THEN_UPDATE,
            grant: GRANT_READ_WRITE_DATA,
            expected_payload: |_pk, marker| json!({ "marker": marker }),
            expected_items: |pk, marker| {
                vec![(
                    pk.to_string(),
                    Some(json!({ "pk": { "S": pk }, "marker": { "S": marker } })),
                )]
            },
        },
        Row {
            case: "an S3 asset package reaching the table through the SDK's resource \
                   interface and reading its own write back",
            stack: "cdk-asset-resource",
            package: Package::S3Asset,
            runtime: "python3.13",
            handler: "writer.main",
            code: RESOURCE_PUT_THEN_GET,
            grant: GRANT_READ_WRITE_DATA,
            expected_payload: |_pk, marker| json!({ "read": marker, "attributes": 2 }),
            expected_items: |pk, marker| {
                vec![(
                    pk.to_string(),
                    Some(json!({ "pk": { "S": pk }, "marker": { "S": marker } })),
                )]
            },
        },
        Row {
            case: "the template's own inline source, writing two items in one batch call",
            stack: "cdk-inline-batch",
            package: Package::InlineSource,
            runtime: "python3.12",
            handler: "index.handler",
            code: BATCH_WRITE,
            grant: GRANT_WRITE_DATA,
            expected_payload: |_pk, _marker| json!({ "unprocessed": 0 }),
            expected_items: |pk, _marker| {
                (0..2)
                    .map(|index| {
                        let key = format!("{pk}-{index}");
                        let item = json!({
                            "pk": { "S": key },
                            "marker": { "N": index.to_string() },
                        });
                        (key, Some(item))
                    })
                    .collect()
            },
        },
    ]
}

/// The module file one row's `Handler` resolves: `index.handler` is `index.py`.
fn source_file(handler: &str) -> String {
    let stem = handler.split('.').next().unwrap_or("index");
    format!("{stem}.py")
}

/// Stage one row's asset in the bootstrap bucket, as `cdk deploy` uploads it, and answer the
/// key it was stored under.
async fn stage_asset(router: &MultiServiceRouter, row: &Row) -> String {
    let bucket = assets_bucket();
    let created = s3(router, "PUT", &format!("/{bucket}"), b"").await;
    assert_eq!(
        created.status,
        200,
        "case {:?}: the assets bucket must be created first, got {} {}",
        row.case,
        created.status,
        created.body
    );
    let key = format!("assets/{}.zip", row.stack);
    let staged = s3(
        router,
        "PUT",
        &format!("/{bucket}/{key}"),
        &package(&source_file(row.handler), row.code),
    )
    .await;
    assert_eq!(
        staged.status,
        200,
        "case {:?}: the asset {key} must be staged first, got {} {}",
        row.case,
        staged.status,
        staged.body
    );
    key
}

/// The `Code` property one row declares: the staged asset's location, or the source inline.
async fn function_code(router: &MultiServiceRouter, row: &Row) -> Value {
    match row.package {
        Package::S3Asset => {
            let key = stage_asset(router, row).await;
            json!({ "S3Bucket": assets_bucket(), "S3Key": key })
        }
        Package::InlineSource => json!({ "ZipFile": row.code }),
    }
}

/// The template one row deploys: the table, the function whose environment carries its name,
/// the role and the inline policy its grant produces, the metadata resource and its condition,
/// and the bootstrap-version parameter with the rule over it — the sections a synthesized CDK
/// stack carries — plus the two outputs the app publishes.
fn cdk_template(row: &Row, code: &Value) -> String {
    json!({
        "Parameters": {
            "BootstrapVersion": {
                "Type": "AWS::SSM::Parameter::Value<String>",
                "Default": BOOTSTRAP_VERSION_PARAMETER,
                "Description": "Version of the CDK Bootstrap resources in this environment",
            },
        },
        "Rules": {
            "CheckBootstrapVersion": {
                "Assertions": [{
                    "Assert": {"Fn::Not": [{"Fn::Contains": [
                        ["1", "2", "3", "4", "5"], {"Ref": "BootstrapVersion"},
                    ]}]},
                    "AssertDescription":
                        "CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' \
                         with a recent version of the CDK CLI.",
                }],
            },
        },
        "Conditions": {
            METADATA_CONDITION: {"Fn::Or": [
                {"Fn::Equals": [{"Ref": "AWS::Region"}, REGION]},
                {"Fn::Equals": [{"Ref": "AWS::Region"}, "eu-west-1"]},
            ]},
        },
        "Resources": {
            TABLE_LOGICAL_ID: {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "KeySchema": [{"AttributeName": PARTITION_KEY, "KeyType": "HASH"}],
                    "AttributeDefinitions": [
                        {"AttributeName": PARTITION_KEY, "AttributeType": "S"},
                    ],
                    "BillingMode": "PAY_PER_REQUEST",
                },
                "UpdateReplacePolicy": "Delete",
                "DeletionPolicy": "Delete",
                "Metadata": {"aws:cdk:path": format!("{}/Table/Resource", row.stack)},
            },
            ROLE_LOGICAL_ID: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": "sts:AssumeRole",
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                        }],
                    },
                },
            },
            POLICY_LOGICAL_ID: {
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": row.grant,
                            "Effect": "Allow",
                            "Resource": [{"Fn::GetAtt": [TABLE_LOGICAL_ID, "Arn"]}],
                        }],
                    },
                    "PolicyName": POLICY_LOGICAL_ID,
                    "Roles": [{"Ref": ROLE_LOGICAL_ID}],
                },
            },
            FUNCTION_LOGICAL_ID: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Code": code,
                    "Environment": {"Variables": {"TABLE": {"Ref": TABLE_LOGICAL_ID}}},
                    "Handler": row.handler,
                    "Role": {"Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"]},
                    "Runtime": row.runtime,
                    "Timeout": FUNCTION_TIMEOUT,
                },
                "DependsOn": [POLICY_LOGICAL_ID, ROLE_LOGICAL_ID],
                "Metadata": {"aws:cdk:path": format!("{}/Writer/Resource", row.stack)},
            },
            METADATA_LOGICAL_ID: {
                "Type": "AWS::CDK::Metadata",
                "Properties": {"Analytics": ANALYTICS},
                "Condition": METADATA_CONDITION,
            },
        },
        "Outputs": {
            "TableName": {"Value": {"Ref": TABLE_LOGICAL_ID}},
            "FunctionName": {"Value": {"Ref": FUNCTION_LOGICAL_ID}},
        },
    })
    .to_string()
}

/// The `OutputValue` the stack reports for `key`, read INSIDE the `Outputs` block so a member
/// of another list in the envelope cannot be swept up with it.
fn output(xml: &str, key: &str) -> Option<String> {
    let block = xml_blocks(xml, "Outputs").into_iter().next()?;
    xml_blocks(&block, "member")
        .into_iter()
        .find(|entry| xml_text(entry, "OutputKey").as_deref() == Some(key))
        .and_then(|entry| xml_text(&entry, "OutputValue"))
}

/// Deploy one row the way the CDK CLI deploys a stack — a `CREATE` change set over the
/// synthesized template, then `ExecuteChangeSet` — and answer the `(function, table)` names
/// the completed stack published.
async fn deploy(router: &MultiServiceRouter, row: &Row) -> (String, String) {
    let case = row.case;
    put_parameter(
        router,
        case,
        BOOTSTRAP_VERSION_PARAMETER,
        "String",
        BOOTSTRAP_VERSION,
    )
    .await;
    let code = function_code(router, row).await;
    let change_set = format!("{}-changeset", row.stack);
    let (status, body) = create_change_set(
        router,
        row.stack,
        &change_set,
        &acknowledging_iam(&[
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &cdk_template(row, &code)),
        ]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateChangeSet, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = execute_change_set(router, &change_set, Some(row.stack)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ExecuteChangeSet, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, described) = describe_stack(router, row.stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} {described}"
    );
    let stack_status = xml_text(&described, "StackStatus");
    assert_eq!(
        stack_status.as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: expected the executed change set to complete the stack, got \
         {stack_status:?} because {:?}",
        failure_reasons(router, row.stack).await
    );

    let function = output(&described, "FunctionName").unwrap_or_default();
    let table = output(&described, "TableName").unwrap_or_default();
    assert!(
        !function.is_empty() && !table.is_empty(),
        "case {case:?}: expected the stack to publish the function and the table it deployed, \
         got {function:?} and {table:?}"
    );
    (function, table)
}

/// Wait until AWS Lambda first reports `name` `Active`, and answer the state it reported then:
/// a created function is `Pending` until that service's own activation span falls due, and
/// "you cannot invoke … while the function is in the Pending state".
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let mut last = String::new();
    for _ in 0..LOOKS {
        let answered = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &[],
            &[],
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected AWS Lambda to hold the function {name:?}, got {} with \
             body {}",
            answered.status, answered.body
        );
        last = answered
            .body
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if last == "Active" {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    last
}

/// Wait until Amazon DynamoDB first reports `table` `ACTIVE`, and answer the status it
/// reported then: "you can perform read and write operations only on an ACTIVE table".
async fn wait_until_table_active(router: &MultiServiceRouter, table: &str) -> String {
    let mut last = String::new();
    for _ in 0..LOOKS {
        last = describe_table(router, table)
            .await
            .and_then(|described| {
                described
                    .get("TableStatus")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if last == "ACTIVE" {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    last
}

/// The item Amazon DynamoDB holds under `key`, read through its own `GetItem`.
async fn held_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
) -> Option<Value> {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { PARTITION_KEY: { "S": key } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetItem for {key:?}, got {status} with body \
         {answered}"
    );
    answered.get("Item").cloned()
}

/// One deploy, one invocation and one read-back per row.
#[tokio::test]
async fn a_cdk_deployed_function_runs_the_sdk_the_runtime_supplies_and_writes_its_table() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let router = router();
        let (function, table) = deploy(&router, &row).await;

        let state = wait_until_active(&router, case, &function).await;
        assert_eq!(
            state, "Active",
            "case {case:?}: expected AWS Lambda to report {function} Active before an \
             invocation, got {state:?}"
        );
        let table_status = wait_until_table_active(&router, &table).await;
        assert_eq!(
            table_status, "ACTIVE",
            "case {case:?}: expected DynamoDB to report {table} ACTIVE, got {table_status:?}"
        );

        let key = format!("item-{index}");
        let marker = format!("written-by-row-{index}");
        let event = json!({ PARTITION_KEY: key, MARKER_ATTRIBUTE: marker });
        let answered = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            event.to_string().as_bytes(),
            &[("x-amz-invocation-type", "RequestResponse")],
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {} with payload {}",
            answered.status, answered.body
        );
        // No `X-Amz-Function-Error`: the header is set only when the handler itself faulted,
        // which a handler whose runtime supplies the SDK it imports does not.
        assert_eq!(
            answered.function_error(),
            None,
            "case {case:?}: expected the handler to run to completion, got the error document \
             {}",
            answered.body
        );
        let expected = (row.expected_payload)(&key, &marker);
        assert_eq!(
            answered.body, expected,
            "case {case:?}: expected the handler's own result {expected}, got {}",
            answered.body
        );

        // What the TABLE holds: the work the handler reported is work DynamoDB really did.
        for (held_key, wanted) in (row.expected_items)(&key, &marker) {
            let held = held_item(&router, case, &table, &held_key).await;
            assert_eq!(
                held, wanted,
                "case {case:?}: expected the table to hold {wanted:?} under {held_key:?}, got \
                 {held:?}"
            );
        }
    }
}
