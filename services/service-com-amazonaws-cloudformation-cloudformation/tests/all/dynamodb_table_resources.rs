// SPDX-License-Identifier: CC0-1.0

//! `AWS::DynamoDB::Table` end to end: the table a stack declares exists in Amazon DynamoDB
//! itself, and the stack publishes that service's OWN identifiers for it.
//!
//! RULE: a declared resource of a type this emulator provisions is created in the service
//! that owns it, and the attributes `Fn::GetAtt` answers for it are the identifiers that
//! service reported — not values CloudFormation computed. For `AWS::DynamoDB::Table` those
//! are the two the type publishes: `Arn`, and the `StreamArn` DynamoDB MINTED when the
//! declaration's `StreamSpecification` enabled a stream. So every declared output over a
//! provisioned table carries an `OutputValue` a client can use, which is what the standard
//! `{o["OutputKey"]: o["OutputValue"]}` read of a stack's outputs needs.
//!
//! The rows differ along what the rule generalises over: each `StreamViewType` the model
//! declares, a table whose declaration enables NO stream (which therefore has no stream ARN
//! at all), whether the template names the table or leaves CloudFormation to name it, the
//! capacity mode, and whether the key schema carries a sort key.
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the DynamoDB calls as a client's SDK makes them — so what is asserted is what a client
//! that deploys a table by template and then reads it observes.

use serde_json::{json, Value};

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts, ACCOUNT_ID, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// The awsJson version Amazon DynamoDB's model declares (`awsJson1_0`).
const DYNAMODB_JSON: &str = "application/x-amz-json-1.0";

/// Call one DynamoDB operation through the same router, signed for `dynamodb` the way a
/// client's SDK signs it: the status and the parsed answer document.
async fn dynamodb(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://dynamodb.{REGION}.amazonaws.com/"))
        .header("content-type", DYNAMODB_JSON)
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dynamodb/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed DynamoDB request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, document)
}

/// The `Table` description DynamoDB answers `DescribeTable` with, or `None` where that
/// service holds no such table.
async fn describe_table(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, document) = dynamodb(router, "DescribeTable", &json!({"TableName": name})).await;
    if status == 200 {
        return document.get("Table").cloned();
    }
    None
}

/// Every output a `DescribeStacks` response reports, as `(OutputKey, OutputValue)` pairs
/// read INSIDE each `<member>` block — so an output that carries NO `OutputValue` member at
/// all is visible as `None` rather than shifting the pairing of two flat lists.
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

/// The `OutputValue` the stack reports for `key`, or `None` where the output carries none.
fn output(xml: &str, key: &str) -> Option<String> {
    outputs(xml)
        .into_iter()
        .find(|(output_key, _)| output_key == key)
        .and_then(|(_, value)| value)
}

/// Every resource failure the stack recorded, as the reason each one carries: what a case
/// whose stack did not complete needs in order to say WHY.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_blocks(&body, "member")
        .into_iter()
        .filter_map(|event| xml_text(&event, "ResourceStatusReason"))
        .collect()
}

/// One row of the table: a declared table, and what the stack must publish for it.
struct Row<'a> {
    case: &'a str,
    stack: &'a str,
    /// The `AWS::DynamoDB::Table` properties the template declares.
    properties: Value,
    /// The table name the template declares, or `None` where CloudFormation names it.
    declared_name: Option<&'a str>,
    /// The `StreamViewType` the declaration enables a stream in, or `None` for a table
    /// whose declaration enables none.
    stream_view_type: Option<&'a str>,
}

/// The `StreamViewType` values the model declares, in the order `StreamSpecification`'s own
/// enum states them (`smithy_query(query="shape",shape="StreamViewType")`).
const STREAM_VIEW_TYPES: &[&str] = &["NEW_IMAGE", "OLD_IMAGE", "NEW_AND_OLD_IMAGES", "KEYS_ONLY"];

/// A `KeySchema`/`AttributeDefinitions` pair over a partition key, and optionally a sort
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

/// One row's declared properties: the key schema, the capacity mode, the stream the row
/// asks for, and the name where the row declares one.
fn properties(
    declared_name: Option<&str>,
    sort_key: bool,
    pay_per_request: bool,
    stream_view_type: Option<&str>,
) -> Value {
    let (schema, definitions) = key_schema(sort_key);
    let mut declared = json!({"KeySchema": schema, "AttributeDefinitions": definitions});
    let members = declared
        .as_object_mut()
        .expect("the declaration is an object");
    if let Some(name) = declared_name {
        members.insert("TableName".to_string(), json!(name));
    }
    if pay_per_request {
        members.insert("BillingMode".to_string(), json!("PAY_PER_REQUEST"));
    } else {
        members.insert(
            "ProvisionedThroughput".to_string(),
            json!({"ReadCapacityUnits": 5, "WriteCapacityUnits": 5}),
        );
    }
    match stream_view_type {
        Some(view_type) => {
            members.insert(
                "StreamSpecification".to_string(),
                json!({"StreamEnabled": true, "StreamViewType": view_type}),
            );
        }
        None => {
            members.insert(
                "StreamSpecification".to_string(),
                json!({"StreamEnabled": false}),
            );
        }
    }
    declared
}

fn rows() -> Vec<Row<'static>> {
    let mut rows = Vec::new();
    // One row per `StreamViewType` the model declares, each with a different shape of
    // declaration around it, so the rule is not read off one spelling.
    for (index, view_type) in STREAM_VIEW_TYPES.iter().enumerate() {
        let named = index % 2 == 0;
        rows.push(Row {
            case: view_type,
            stack: match *view_type {
                "NEW_IMAGE" => "ddb-new-image",
                "OLD_IMAGE" => "ddb-old-image",
                "NEW_AND_OLD_IMAGES" => "ddb-both-images",
                _ => "ddb-keys-only",
            },
            properties: properties(
                named.then_some("declared-table-name"),
                index % 2 == 1,
                index < 2,
                Some(view_type),
            ),
            declared_name: named.then_some("declared-table-name"),
            stream_view_type: Some(view_type),
        });
    }
    // And a table whose declaration leaves the stream OFF: it has no stream, so there is no
    // stream ARN for an output to carry.
    rows.push(Row {
        case: "a table whose declaration enables no stream",
        stack: "ddb-no-stream",
        properties: properties(Some("stream-less-table"), false, true, None),
        declared_name: Some("stream-less-table"),
        stream_view_type: None,
    });
    rows
}

/// The template one row deploys: the table, and the three outputs a client reads it through.
fn template(row: &Row<'_>) -> String {
    json!({
        "Resources": {"Table": {"Type": "AWS::DynamoDB::Table",
                                "Properties": row.properties.clone()}},
        "Outputs": {
            "TableName": {"Value": {"Ref": "Table"}},
            "TableArn": {"Value": {"Fn::GetAtt": ["Table", "Arn"]}},
            "StreamArn": {"Value": {"Fn::GetAtt": ["Table", "StreamArn"]}},
        },
    })
    .to_string()
}

/// A table a stack declares is created in DynamoDB, and the stack's outputs publish the
/// identifiers that service answered with — the name, the table ARN, and the stream ARN of
/// the stream the declaration enabled.
#[tokio::test]
async fn a_declared_table_exists_in_dynamodb_and_its_identifiers_are_the_services_own() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let (status, body) = create_stack(&router, row.stack, &template(&row), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} {body}"
        );
        wait_for_the_transition().await;
        let (status, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} {described}"
        );
        let stack_status = xml_text(&described, "StackStatus");
        assert_eq!(
            stack_status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {stack_status:?} because \
             {:?}",
            failure_reasons(&router, row.stack).await
        );

        // The name the stack publishes is the one DynamoDB holds the table under.
        let name = output(&described, "TableName").unwrap_or_default();
        if let Some(declared) = row.declared_name {
            assert_eq!(
                name, declared,
                "case {case:?}: expected Ref to answer the declared table name, got {name:?}"
            );
        }
        let table = describe_table(&router, &name).await;
        let Some(table) = table else {
            panic!(
                "case {case:?}: expected DynamoDB to hold the table {name:?} the stack \
                 reports created, got no table at all"
            );
        };

        // The table ARN the stack publishes is the one DynamoDB reports for it.
        let table_arn = output(&described, "TableArn");
        assert_eq!(
            table_arn.as_deref(),
            table.get("TableArn").and_then(Value::as_str),
            "case {case:?}: expected the stack's Arn output to be the ARN DynamoDB reports, \
             got {table_arn:?} against {:?}",
            table.get("TableArn")
        );
        assert_eq!(
            table_arn.as_deref(),
            Some(format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/{name}").as_str()),
            "case {case:?}: expected the documented table ARN, got {table_arn:?}"
        );

        // The key schema the declaration asked for is the one the table has, which is what
        // says the create carried the declaration over rather than inventing a table.
        let (schema, definitions) = key_schema(
            row.properties["KeySchema"]
                .as_array()
                .is_some_and(|keys| keys.len() == 2),
        );
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

        // And the stream: the ARN the stack publishes is the LatestStreamArn DynamoDB
        // minted, reported only for a table that HAS a stream.
        let stream_arn = output(&described, "StreamArn");
        match row.stream_view_type {
            Some(view_type) => {
                assert_eq!(
                    table
                        .get("StreamSpecification")
                        .and_then(|specification| specification.get("StreamViewType"))
                        .and_then(Value::as_str),
                    Some(view_type),
                    "case {case:?}: expected the table to capture in {view_type:?}, got {:?}",
                    table.get("StreamSpecification")
                );
                assert_eq!(
                    stream_arn.as_deref(),
                    table.get("LatestStreamArn").and_then(Value::as_str),
                    "case {case:?}: expected the stack's StreamArn output to be the stream \
                     ARN DynamoDB minted, got {stream_arn:?} against {:?}",
                    table.get("LatestStreamArn")
                );
                let published = stream_arn.unwrap_or_default();
                assert!(
                    published.starts_with(&format!(
                        "arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/{name}/stream/"
                    )),
                    "case {case:?}: expected a stream ARN nested under the table's own, got \
                     {published:?}"
                );
            }
            // A table whose declaration enabled no stream has no stream ARN: the attribute
            // is reported absent rather than invented, which is what a client must never be
            // handed.
            None => {
                assert_eq!(
                    stream_arn, None,
                    "case {case:?}: expected no StreamArn for a table with no stream, got \
                     {stream_arn:?}"
                );
                assert_eq!(
                    table.get("LatestStreamArn"),
                    None,
                    "case {case:?}: expected DynamoDB to report no stream for the table \
                     either, got {:?}",
                    table.get("LatestStreamArn")
                );
            }
        }
    }
}

/// Deleting the stack deletes the table again: "deleting a stack deletes the resources in
/// it", so DynamoDB no longer holds it afterwards.
#[tokio::test]
async fn deleting_the_stack_deletes_the_table_from_dynamodb() {
    let router = router();
    let case = "a stream-enabled table deleted with its stack";
    let stack = "ddb-deleted";
    let row = Row {
        case,
        stack,
        properties: properties(Some("deleted-table"), false, true, Some("KEYS_ONLY")),
        declared_name: Some("deleted-table"),
        stream_view_type: Some("KEYS_ONLY"),
    };
    let (status, body) = create_stack(&router, stack, &template(&row), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} {body}"
    );
    wait_for_the_transition().await;
    assert!(
        describe_table(&router, "deleted-table").await.is_some(),
        "case {case:?}: expected DynamoDB to hold the table the stack created, got none \
         because {:?}",
        failure_reasons(&router, stack).await
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} {body}"
    );
    // The stack's own transition, then the table's: DynamoDB reports a deleted table
    // `DELETING` until its own span falls due, and the stack's deletion is not finished
    // while that service still holds it.
    for _ in 0..12 {
        if describe_table(&router, "deleted-table").await.is_none() {
            break;
        }
        wait_for_the_transition().await;
    }
    let held = describe_table(&router, "deleted-table").await;
    assert!(
        held.is_none(),
        "case {case:?}: expected DynamoDB to hold no table once the stack's deletion \
         finished, got {held:?}"
    );
}

/// A property of the type this emulator does not apply fails the resource rather than being
/// dropped: a table reported created with less than its declaration asked for is the
/// shallow success the provisioner refuses to report.
#[tokio::test]
async fn a_property_the_provisioner_does_not_apply_fails_the_resource() {
    // The rows are properties `CreateTable` does not take at all: each is a DynamoDB call of
    // its own, made on a table that already exists, which nothing here makes.
    let cases: &[(&str, &str, Value)] = &[
        (
            "time to live",
            "ddb-unapplied-ttl",
            json!({"AttributeName": "expiresAt", "Enabled": true}),
        ),
        (
            "point-in-time recovery",
            "ddb-unapplied-pitr",
            json!({"PointInTimeRecoveryEnabled": true}),
        ),
        (
            "a Kinesis destination",
            "ddb-unapplied-kinesis",
            json!({"StreamArn": format!("arn:aws:kinesis:{REGION}:{ACCOUNT_ID}:stream/changes")}),
        ),
    ];
    let unapplied = [
        "TimeToLiveSpecification",
        "PointInTimeRecoverySpecification",
        "KinesisStreamSpecification",
    ];
    for ((case, stack, value), property) in cases.iter().zip(unapplied) {
        let router = router();
        let mut declared = properties(Some("unapplied-table"), false, true, Some("KEYS_ONLY"));
        declared
            .as_object_mut()
            .expect("the declaration is an object")
            .insert(property.to_string(), value.clone());
        let body = json!({
            "Resources": {"Table": {"Type": "AWS::DynamoDB::Table",
                                    "Properties": declared}},
        })
        .to_string();
        let (status, answer) = create_stack(&router, stack, &body, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} {answer}"
        );
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, stack).await;
        let stack_status = xml_text(&described, "StackStatus");
        assert_eq!(
            stack_status.as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than report \
             a table created without its {property}, got {stack_status:?} in {described}"
        );
        let reasons = failure_reasons(&router, stack).await;
        assert!(
            reasons.iter().any(|reason| reason.contains(property)),
            "case {case:?}: expected a failure naming the {property} property, got {reasons:?}"
        );
        assert!(
            describe_table(&router, "unapplied-table").await.is_none(),
            "case {case:?}: expected DynamoDB to hold no table for a create that failed"
        );
    }
}

/// Every declared output of the completed stack carries an `OutputValue`, so the standard
/// `{o["OutputKey"]: o["OutputValue"]}` read of a stack's outputs cannot raise.
#[tokio::test]
async fn every_output_over_a_provisioned_table_carries_a_value() {
    let router = router();
    let case = "a stream-enabled table whose outputs are its name, its ARN and its stream";
    let stack = "ddb-outputs";
    let row = Row {
        case,
        stack,
        properties: properties(None, true, true, Some("NEW_AND_OLD_IMAGES")),
        declared_name: None,
        stream_view_type: Some("NEW_AND_OLD_IMAGES"),
    };
    let (status, body) = create_stack(&router, stack, &template(&row), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, stack).await;
    let reported = outputs(&described);
    assert_eq!(
        reported.len(),
        3,
        "case {case:?}: expected all three declared outputs, got {reported:?}"
    );
    for (key, value) in &reported {
        assert!(
            value.as_deref().is_some_and(|value| !value.is_empty()),
            "case {case:?}: output {key:?} carries no OutputValue at all ({reported:?}); the \
             standard {{o[\"OutputKey\"]: o[\"OutputValue\"]}} read raises here"
        );
    }
    // The keys are the ones the template declared, whatever order the response lists them
    // in.
    let keys: Vec<&str> = reported.iter().map(|(key, _)| key.as_str()).collect();
    for declared in ["TableName", "TableArn", "StreamArn"] {
        assert!(
            keys.contains(&declared),
            "case {case:?}: expected the declared output {declared:?}, got {keys:?}"
        );
    }
    // The resource the stack reports created is the table DynamoDB holds, addressed by the
    // generated name the stack published.
    let name = output(&described, "TableName").unwrap_or_default();
    assert!(
        describe_table(&router, &name).await.is_some(),
        "case {case:?}: expected DynamoDB to hold the generated table {name:?}"
    );
    let physical = xml_texts(&described, "StackStatus");
    assert_eq!(
        physical.first().map(String::as_str),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {physical:?} because {:?}",
        failure_reasons(&router, stack).await
    );
}
