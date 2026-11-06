// SPDX-License-Identifier: CC0-1.0

//! A stack deployed the way the AWS CDK deploys one — a `CREATE` change set, then
//! `ExecuteChangeSet` — really creates its declared `AWS::DynamoDB::Table` in Amazon
//! DynamoDB.
//!
//! RULE under test: **a stack reports `CREATE_COMPLETE` only once every declared resource
//! of a type this emulator provisions is HELD by the service that owns it, under the
//! physical resource id the stack publishes — whichever operation deployed the template,
//! `CreateStack` or the execution of a change set — so the value a `Ref`/output over an
//! `AWS::DynamoDB::Table` resolves to is a name `DescribeTable` answers for, with the key
//! schema, the attribute definitions and the capacity mode the declaration asked for; and
//! deleting the stack takes that table away again.**
//!
//! `dynamodb_table_resources.rs` holds the same rule for the `CreateStack` path and for the
//! identifiers a table publishes; this suite is the CHANGE-SET path of it — the one the CDK
//! CLI uses — and the one whose templates carry the rest of a synthesized CDK stack around
//! the table (the bootstrap-version parameter and its rule, the metadata resource and its
//! condition, the function and the role and policy its grant produces), so a table is not
//! read as provisioned only when it is the only thing in the template.
//!
//! The rows differ along what the rule generalises over: the capacity mode (on-demand and
//! provisioned), whether the key schema carries a sort key, whether the template NAMES the
//! table or leaves CloudFormation to name it, whether a stream is enabled, which of the
//! create's other members the declaration carries (tags, a table class), and whether the
//! change set was given the template INLINE or by reference to an Amazon S3 object — the
//! form the CDK CLI uses, which stages the synthesized template in the bootstrap bucket
//! first.
//!
//! Every expected value is computed from what the row declares, except the GENERATED name of
//! a row that declares none — which is CloudFormation's to compute, and what is asserted
//! about it is its published form and that DynamoDB holds a table under it.

use serde_json::{json, Value};

use crate::change_set_lifecycle::{create_change_set, execute_change_set};
use crate::dynamic_references::put_parameter;
use crate::dynamodb_table_resources::describe_table;
use crate::stack_lifecycle::{
    acknowledging_iam, call, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, REGION,
};
use crate::template_by_reference::{create_staging_bucket, stage, STAGING_BUCKET};

/// The logical ids the CDK gives the constructs of the app this suite is written for: a
/// `ddb.Table`, a `_lambda.Function` and the role and inline policy its grant produces, plus
/// the metadata resource the CDK adds to every synthesized stack.
const TABLE_LOGICAL_ID: &str = "TableCD117FA1";
const ROLE_LOGICAL_ID: &str = "FnServiceRole7D7F0988";
const POLICY_LOGICAL_ID: &str = "FnServiceRoleDefaultPolicyA2BD6A50";
const FUNCTION_LOGICAL_ID: &str = "Fn9270CBC0";
const METADATA_LOGICAL_ID: &str = "CDKMetadata";

/// The name of the condition the CDK guards its metadata resource with.
const METADATA_CONDITION: &str = "CDKMetadataAvailable";

/// A property value of the form the CDK encodes its version report in.
const ANALYTICS: &str = "v2:deflate64:H4sIAAAAAAAA";

/// The Parameter Store parameter a `DefaultStackSynthesizer` stack reads the bootstrap
/// version from, and a version outside the range its own rule rejects.
const BOOTSTRAP_VERSION_PARAMETER: &str = "/cdk-bootstrap/hnb659fds/version";
const BOOTSTRAP_VERSION: &str = "21";

/// The handler the deployed function carries, as the app inlines it.
const HANDLER_SOURCE: &str = "import os\n\ndef handler(event, context):\n    return event\n";

/// The `StackStatus` the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// One row: the table one CDK-shaped stack declares, and what the deployment owes for it.
struct Row<'a> {
    case: &'a str,
    stack: &'a str,
    /// The name the template declares for the table, or `None` where CloudFormation names
    /// it.
    declared_name: Option<&'a str>,
    /// Whether the key schema carries a sort key beside the partition key.
    sort_key: bool,
    /// Whether the declaration asks for on-demand capacity.
    pay_per_request: bool,
    /// The `StreamViewType` the declaration enables a stream in, if any.
    stream_view_type: Option<&'a str>,
    /// The tag set the declaration carries, as `(Key, Value)` pairs.
    tags: &'a [(&'a str, &'a str)],
    /// The `TableClass` the declaration carries, if any.
    table_class: Option<&'a str>,
    /// Whether the change set is given the template by REFERENCE — staged in Amazon S3 and
    /// addressed by `TemplateURL`, as the CDK CLI submits it — rather than inline.
    by_reference: bool,
}

/// The read and write capacity a provisioned row declares.
const READ_CAPACITY: u64 = 3;
const WRITE_CAPACITY: u64 = 4;

/// A `(KeySchema, AttributeDefinitions)` pair over a partition key, and optionally a sort
/// key.
fn key_schema(sort_key: bool) -> (Value, Value) {
    if sort_key {
        return (
            json!([
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ]),
            json!([
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
            ]),
        );
    }
    (
        json!([{"AttributeName": "pk", "KeyType": "HASH"}]),
        json!([{"AttributeName": "pk", "AttributeType": "S"}]),
    )
}

/// The `AWS::DynamoDB::Table` properties one row declares.
fn table_properties(row: &Row<'_>) -> Value {
    let (schema, definitions) = key_schema(row.sort_key);
    let mut declared = json!({"KeySchema": schema, "AttributeDefinitions": definitions});
    let members = declared
        .as_object_mut()
        .expect("the declaration is an object");
    if let Some(name) = row.declared_name {
        members.insert("TableName".to_string(), json!(name));
    }
    if row.pay_per_request {
        members.insert("BillingMode".to_string(), json!("PAY_PER_REQUEST"));
    } else {
        members.insert(
            "ProvisionedThroughput".to_string(),
            json!({"ReadCapacityUnits": READ_CAPACITY, "WriteCapacityUnits": WRITE_CAPACITY}),
        );
    }
    if let Some(view_type) = row.stream_view_type {
        members.insert(
            "StreamSpecification".to_string(),
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
        );
    }
    if let Some(table_class) = row.table_class {
        members.insert("TableClass".to_string(), json!(table_class));
    }
    if !row.tags.is_empty() {
        members.insert(
            "Tags".to_string(),
            Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            ),
        );
    }
    declared
}

/// The template one row deploys: the table, the function whose environment carries its name,
/// the role and inline policy the grant produces, the metadata resource and its condition,
/// the bootstrap-version parameter and the rule over it — the sections a synthesized CDK
/// stack carries — and the two outputs the app publishes.
fn cdk_template(row: &Row<'_>) -> String {
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
                "Properties": table_properties(row),
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
                            "Action": [
                                "dynamodb:BatchGetItem",
                                "dynamodb:GetItem",
                                "dynamodb:PutItem",
                                "dynamodb:Query",
                                "dynamodb:Scan",
                                "dynamodb:UpdateItem",
                            ],
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
                    "Code": {"ZipFile": HANDLER_SOURCE},
                    "Environment": {"Variables": {"TABLE": {"Ref": TABLE_LOGICAL_ID}}},
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"]},
                    "Runtime": "python3.12",
                },
                "DependsOn": [POLICY_LOGICAL_ID, ROLE_LOGICAL_ID],
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

fn rows() -> Vec<Row<'static>> {
    vec![
        // The shape the app in the recorded deploy synthesizes: on-demand capacity, a
        // partition key alone, and no name of its own.
        Row {
            case: "the default CDK table: on-demand, a generated name",
            stack: "cdk-table-on-demand",
            declared_name: None,
            sort_key: false,
            pay_per_request: true,
            stream_view_type: None,
            tags: &[],
            table_class: None,
            by_reference: true,
        },
        // Provisioned capacity, a sort key, and still a generated name.
        Row {
            case: "a provisioned table with a sort key",
            stack: "cdk-table-provisioned",
            declared_name: None,
            sort_key: true,
            pay_per_request: false,
            stream_view_type: None,
            tags: &[],
            table_class: None,
            by_reference: false,
        },
        // A table the template NAMES, whose declaration also enables a stream.
        Row {
            case: "a named table whose declaration enables a stream",
            stack: "cdk-table-named",
            declared_name: Some("cdk-declared-table"),
            sort_key: false,
            pay_per_request: true,
            stream_view_type: Some("NEW_AND_OLD_IMAGES"),
            tags: &[],
            table_class: None,
            by_reference: false,
        },
        // A declaration carrying the create's other members: a tag set and a table class.
        Row {
            case: "a tagged table in an explicit table class",
            stack: "cdk-table-tagged",
            declared_name: None,
            sort_key: true,
            pay_per_request: true,
            stream_view_type: Some("KEYS_ONLY"),
            tags: &[("Tier", "gold"), ("Owner", "platform")],
            table_class: Some("STANDARD_INFREQUENT_ACCESS"),
            by_reference: true,
        },
    ]
}

/// Every output a `DescribeStacks` response reports, as `(OutputKey, OutputValue)` pairs read
/// INSIDE each `<member>` block.
fn outputs(xml: &str) -> Vec<(String, Option<String>)> {
    let Some(block) = xml_blocks(xml, "Outputs").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .into_iter()
        .filter_map(|output| {
            let key = xml_text(&output, "OutputKey")?;
            Some((key, xml_text(&output, "OutputValue")))
        })
        .collect()
}

/// The `OutputValue` the stack reports for `key`, or `None` where it reports none.
fn output(xml: &str, key: &str) -> Option<String> {
    outputs(xml)
        .into_iter()
        .find(|(output_key, _)| output_key == key)
        .and_then(|(_, value)| value)
}

/// Every resource failure the stack recorded: what a case whose stack did not complete needs
/// in order to say WHY.
async fn failure_reasons(
    router: &smithy_server_runtime::MultiServiceRouter,
    stack: &str,
) -> Vec<String> {
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_blocks(&body, "member")
        .into_iter()
        .filter_map(|event| xml_text(&event, "ResourceStatusReason"))
        .collect()
}

/// Deploy one row the way the CDK CLI deploys a stack: a `CREATE` change set over the
/// synthesized template, then `ExecuteChangeSet`. Answers the `DescribeStacks` body of the
/// settled stack.
async fn deploy_through_a_change_set(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &Row<'_>,
) -> String {
    let case = row.case;
    put_parameter(
        router,
        case,
        BOOTSTRAP_VERSION_PARAMETER,
        "String",
        BOOTSTRAP_VERSION,
    )
    .await;
    let change_set = format!("{}-changeset", row.stack);
    // The template reaches the change set the way the row says a client submitted it: inline,
    // or staged in Amazon S3 and addressed by its URL as the CDK CLI stages it.
    let source = if row.by_reference {
        let key = format!("{}/template.json", row.stack);
        create_staging_bucket(router).await;
        stage(router, &key, &cdk_template(row)).await;
        member(
            "TemplateURL",
            &format!("https://{STAGING_BUCKET}.s3.{REGION}.amazonaws.com/{key}"),
        )
    } else {
        member("TemplateBody", &cdk_template(row))
    };
    let (status, body) = create_change_set(
        router,
        row.stack,
        &change_set,
        &acknowledging_iam(&[member("ChangeSetType", "CREATE"), source]),
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
    described
}

/// The table a CDK-shaped stack declares exists in Amazon DynamoDB under the name the
/// executed change set published, with the configuration the declaration asked for.
#[tokio::test]
async fn a_change_set_execution_creates_the_declared_table_in_dynamodb() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let described = deploy_through_a_change_set(&router, &row).await;

        // The name the stack hands over is the one DynamoDB holds the table under: that
        // hand-off is the whole contract a client reads an output for.
        let name = output(&described, "TableName").unwrap_or_default();
        match row.declared_name {
            Some(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected Ref to answer the declared table name, got {name:?}"
            ),
            None => {
                let prefix = format!("{}-{TABLE_LOGICAL_ID}-", row.stack);
                assert!(
                    name.starts_with(&prefix),
                    "case {case:?}: expected a generated name of the form {prefix}<unique>, \
                     got {name:?}"
                );
            }
        }
        let Some(table) = describe_table(&router, &name).await else {
            panic!(
                "case {case:?}: expected DynamoDB to hold the table {name:?} the completed \
                 stack published, got no table at all"
            );
        };

        // It is reported ready, under that very name.
        assert_eq!(
            table.get("TableName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the description to be of the published table, got {:?}",
            table.get("TableName")
        );
        assert_eq!(
            table.get("TableStatus").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected the table to be ACTIVE, got {:?}",
            table.get("TableStatus")
        );

        // The key schema and the attribute definitions are the declared ones.
        let (schema, definitions) = key_schema(row.sort_key);
        assert_eq!(
            table.get("KeySchema"),
            Some(&schema),
            "case {case:?}: expected the declared key schema, got {:?}",
            table.get("KeySchema")
        );
        assert_eq!(
            table.get("AttributeDefinitions"),
            Some(&definitions),
            "case {case:?}: expected the declared attribute definitions, got {:?}",
            table.get("AttributeDefinitions")
        );

        // And the capacity mode. A table that has been on-demand reports a
        // `BillingModeSummary`; one that has only ever been provisioned reports its
        // throughput and no summary ("You may need to switch to on-demand mode at least once
        // in order to return a BillingModeSummary response").
        let summary = table
            .get("BillingModeSummary")
            .and_then(|summary| summary.get("BillingMode"))
            .and_then(Value::as_str);
        if row.pay_per_request {
            assert_eq!(
                summary,
                Some("PAY_PER_REQUEST"),
                "case {case:?}: expected the on-demand capacity mode the declaration asked \
                 for, got {:?}",
                table.get("BillingModeSummary")
            );
        } else {
            assert_eq!(
                summary, None,
                "case {case:?}: expected no BillingModeSummary for a table that has only \
                 ever been provisioned, got {summary:?}"
            );
            let throughput = table.get("ProvisionedThroughput");
            assert_eq!(
                throughput
                    .and_then(|units| units.get("ReadCapacityUnits"))
                    .and_then(Value::as_u64),
                Some(READ_CAPACITY),
                "case {case:?}: expected the declared read capacity, got {throughput:?}"
            );
            assert_eq!(
                throughput
                    .and_then(|units| units.get("WriteCapacityUnits"))
                    .and_then(Value::as_u64),
                Some(WRITE_CAPACITY),
                "case {case:?}: expected the declared write capacity, got {throughput:?}"
            );
        }

        // The stream the declaration asked for, captured in the view type it named.
        let specification = table.get("StreamSpecification");
        match row.stream_view_type {
            Some(view_type) => assert_eq!(
                specification
                    .and_then(|specification| specification.get("StreamViewType"))
                    .and_then(Value::as_str),
                Some(view_type),
                "case {case:?}: expected the declared stream view type, got \
                 {specification:?}"
            ),
            None => assert!(
                specification.is_none_or(|specification| specification
                    .get("StreamEnabled")
                    .and_then(Value::as_bool)
                    != Some(true)),
                "case {case:?}: expected no stream for a declaration that enabled none, got \
                 {specification:?}"
            ),
        }

        // The other output the app publishes carries a value too: the function the grant was
        // made for is named beside the table it writes.
        let function = output(&described, "FunctionName");
        assert!(
            function.as_deref().is_some_and(|name| !name.is_empty()),
            "case {case:?}: expected the stack to publish the function name, got {function:?}"
        );
    }
}

/// Deleting the stack takes the table away again, which is what the CDK's own
/// `RemovalPolicy.DESTROY` (a `DeletionPolicy` of `Delete`) asks for.
#[tokio::test]
async fn deleting_the_stack_removes_the_table_from_dynamodb() {
    let router = router();
    let row = Row {
        case: "a CDK-deployed table deleted with its stack",
        stack: "cdk-table-deleted",
        declared_name: Some("cdk-deleted-table"),
        sort_key: false,
        pay_per_request: true,
        stream_view_type: None,
        tags: &[],
        table_class: None,
        by_reference: true,
    };
    let case = row.case;
    deploy_through_a_change_set(&router, &row).await;
    assert!(
        describe_table(&router, "cdk-deleted-table").await.is_some(),
        "case {case:?}: expected DynamoDB to hold the table the deploy created, got none \
         because {:?}",
        failure_reasons(&router, row.stack).await
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} {body}"
    );
    // The stack's own transition, then the table's: DynamoDB reports a deleted table
    // `DELETING` until its own span falls due.
    for _ in 0..12 {
        if describe_table(&router, "cdk-deleted-table").await.is_none() {
            break;
        }
        wait_for_the_transition().await;
    }
    let held = describe_table(&router, "cdk-deleted-table").await;
    assert!(
        held.is_none(),
        "case {case:?}: expected DynamoDB to hold no table once the stack's deletion \
         finished, got {held:?}"
    );
}
