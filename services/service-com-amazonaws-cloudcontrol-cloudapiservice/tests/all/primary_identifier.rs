// SPDX-License-Identifier: CC0-1.0

//! Where a resource's primary identifier comes from.
//!
//! Cloud Control API addresses a resource by the identifier its own type declares, so
//! the rule under test is about the resource model as a whole rather than about any one
//! type: the identifier is the value of the type's own identifier property in the
//! desired state, and a desired state naming none gets one the service allocates.

use serde_json::{json, Value};

use super::emulator::{code, message, progress, text, Emulator};

/// One row of the identifier table.
struct Case {
    /// What this row is about.
    name: &'static str,
    /// The `TypeName` the resource is created under.
    type_name: &'static str,
    /// The desired state the create carries.
    desired: Value,
    /// The identifier the service must resolve, or `None` where the desired state names
    /// none and the service allocates one.
    expected: Option<&'static str>,
}

#[tokio::test]
async fn the_primary_identifier_is_the_value_of_the_types_own_identifier_property() {
    let emulator = Emulator::new();
    // The rows differ along what the rule generalises over: which property of the
    // resource model carries the identifier (the type's own name, its id, its ARN, and
    // the bare fallbacks a type uses when the resource's name would only repeat
    // itself), the JSON type of that value, which property wins when several could, and
    // a model that names no identifier at all.
    let rows = [
        Case {
            name: "the type's own name property",
            type_name: "AWS::S3::Bucket",
            desired: json!({ "BucketName": "demo-bucket", "Tags": [] }),
            expected: Some("demo-bucket"),
        },
        Case {
            name: "another type's name property",
            type_name: "AWS::Logs::LogGroup",
            desired: json!({ "LogGroupName": "app-logs", "RetentionInDays": 7 }),
            expected: Some("app-logs"),
        },
        Case {
            name: "the type's own id property, spelled with the acronym as a word",
            type_name: "AWS::EC2::VPC",
            desired: json!({ "VpcId": "vpc-0a1b2c3d", "CidrBlock": "10.0.0.0/16" }),
            expected: Some("vpc-0a1b2c3d"),
        },
        Case {
            name: "the type's own name property, spelled in lower case",
            type_name: "AWS::Custom::Thing",
            desired: json!({ "thingname": "thing-1" }),
            expected: Some("thing-1"),
        },
        Case {
            name: "the type's own ARN property",
            type_name: "AWS::SNS::Topic",
            desired: json!({
                "TopicArn": "arn:aws:sns:us-east-1:123456789012:orders",
                "DisplayName": "Orders",
            }),
            expected: Some("arn:aws:sns:us-east-1:123456789012:orders"),
        },
        Case {
            name: "the bare Name fallback",
            type_name: "AWS::ApiGateway::Stage",
            desired: json!({ "Name": "prod", "TracingEnabled": true }),
            expected: Some("prod"),
        },
        Case {
            name: "the bare Id fallback",
            type_name: "AWS::Custom::Widget",
            desired: json!({ "Id": "widget-7", "Colour": "blue" }),
            expected: Some("widget-7"),
        },
        Case {
            name: "the bare Arn fallback",
            type_name: "AWS::Custom::Gadget",
            desired: json!({ "Arn": "arn:aws:custom:us-east-1:123456789012:gadget/g1" }),
            expected: Some("arn:aws:custom:us-east-1:123456789012:gadget/g1"),
        },
        Case {
            name: "the most specific property wins over the fallbacks",
            type_name: "AWS::S3::Bucket",
            desired: json!({ "BucketName": "primary", "Name": "secondary", "Id": "tertiary" }),
            expected: Some("primary"),
        },
        Case {
            name: "a numeric identifier",
            type_name: "AWS::Custom::Shard",
            desired: json!({ "ShardId": 42, "Partition": "a" }),
            expected: Some("42"),
        },
        Case {
            name: "a model that names no identifier",
            type_name: "AWS::SQS::Queue",
            desired: json!({ "DelaySeconds": 5 }),
            expected: None,
        },
    ];

    // Started together, then waited for: the operations are independent, so one span
    // covers them all.
    let mut accepted = Vec::new();
    for case in &rows {
        let event = emulator
            .create_ok(case.type_name, &case.desired, None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }

    for (case, request_token) in rows.iter().zip(&accepted) {
        let settled = emulator.settled(request_token).await;
        let identifier = text(&settled, "Identifier");
        match case.expected {
            Some(expected) => assert_eq!(
                identifier, expected,
                "case {:?}: expected the identifier {expected:?}, got {identifier:?}",
                case.name
            ),
            None => {
                assert!(
                    !identifier.is_empty(),
                    "case {:?}: the service must allocate an identifier, got {settled:?}",
                    case.name
                );
                // An identifier the service allocated is recorded in the resource model
                // under the type's own id property, so the identifier a caller is handed
                // is always findable as a property of the resource it names.
                let assigned = format!(
                    "{}Id",
                    case.type_name.split("::").nth(2).unwrap_or_default()
                );
                let stored = emulator.properties(case.type_name, &identifier).await;
                assert_eq!(
                    stored.get(&assigned),
                    Some(&json!(identifier)),
                    "case {:?}: the allocated identifier must be in the resource model under \
                     {assigned}, got {stored:?}",
                    case.name
                );
            }
        }

        // Whatever the identifier came from, the resource reads back under it.
        let stored = emulator.properties(case.type_name, &identifier).await;
        for (property, wanted) in case.desired.as_object().cloned().unwrap_or_default() {
            assert_eq!(
                stored.get(&property),
                Some(&wanted),
                "case {:?}: expected {property} to read back as {wanted:?}, got {stored:?}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn two_creates_naming_no_identifier_are_two_resources() {
    let emulator = Emulator::new();
    const QUEUE: &str = "AWS::SQS::Queue";
    let desired = json!({ "DelaySeconds": 5 });

    let first = emulator.create_ok(QUEUE, &desired, None).await;
    let second = emulator.create_ok(QUEUE, &desired, None).await;
    let first = emulator.settled(&text(&first, "RequestToken")).await;
    let second = emulator.settled(&text(&second, "RequestToken")).await;

    let (one, other) = (text(&first, "Identifier"), text(&second, "Identifier"));
    assert_ne!(
        one, other,
        "two allocated identifiers must differ, got {one:?} twice"
    );
    let mut listed = emulator.walk(QUEUE, None).await;
    listed.sort();
    let mut expected = vec![one, other];
    expected.sort();
    assert_eq!(
        listed, expected,
        "both resources must be listed, expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn the_same_identifier_under_two_types_is_two_resources() {
    let emulator = Emulator::new();
    const BUCKET: &str = "AWS::S3::Bucket";
    const LOG_GROUP: &str = "AWS::Logs::LogGroup";
    let bucket = json!({ "BucketName": "shared", "Tags": [] });
    let log_group = json!({ "LogGroupName": "shared", "RetentionInDays": 7 });

    assert_eq!(emulator.provision(BUCKET, &bucket).await, "shared");
    assert_eq!(emulator.provision(LOG_GROUP, &log_group).await, "shared");
    assert_eq!(
        emulator.properties(BUCKET, "shared").await,
        bucket,
        "each type must report its own resource model"
    );
    assert_eq!(
        emulator.properties(LOG_GROUP, "shared").await,
        log_group,
        "each type must report its own resource model"
    );

    // Deleting one leaves the other, which is what makes them two resources.
    emulator.delete_and_settle(BUCKET, "shared").await;
    let (status, body) = emulator.get(BUCKET, "shared").await;
    assert_eq!(
        status, 404,
        "the deleted resource: expected HTTP 404, got {status} with body {body:?}"
    );
    let (status, body) = emulator.get(LOG_GROUP, "shared").await;
    assert_eq!(
        status, 200,
        "the other type's resource must survive, got {status} with body {body:?}"
    );
}

#[tokio::test]
async fn an_identifier_is_accepted_at_the_modelled_bound_and_refused_past_it() {
    let emulator = Emulator::new();
    const WIDGET: &str = "AWS::Custom::Widget";

    // The model gives `Identifier` a length of 1..1024, so both published bounds are
    // exercised, and then one character past the maximum. The creates are independent,
    // so they are started together and waited for once.
    let lengths = [1_usize, 2, 1023, 1024];
    let mut accepted = Vec::new();
    for length in lengths {
        let event = emulator
            .create_ok(WIDGET, &json!({ "WidgetName": "b".repeat(length) }), None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }
    for (length, request_token) in lengths.iter().zip(&accepted) {
        let settled = emulator.settled(request_token).await;
        let identifier = text(&settled, "Identifier");
        let name = "b".repeat(*length);
        assert_eq!(
            identifier, name,
            "length {length}: expected the identifier the desired state names, got \
             {identifier:?}"
        );
        let stored = emulator.properties(WIDGET, &identifier).await;
        assert_eq!(
            stored.get("WidgetName"),
            Some(&json!(name)),
            "length {length}: the resource must read back under that identifier, got {stored:?}"
        );
    }

    let too_long = "b".repeat(1025);
    let (status, body) = emulator
        .create(WIDGET, &json!({ "WidgetName": too_long }), None)
        .await;
    assert_eq!(
        status, 400,
        "one character past the maximum: expected HTTP 400, got {status} with body {body:?}"
    );
    assert!(
        code(&body).contains("InvalidRequestException"),
        "one character past the maximum: expected InvalidRequestException, got {:?}",
        code(&body)
    );
    assert_eq!(
        message(&body),
        "Invalid request provided: WidgetName must be a non-empty scalar of at most 1024 \
         characters to identify the resource",
        "one character past the maximum: unexpected wording"
    );
    assert!(
        progress(&body).is_null(),
        "a refused create reports no ProgressEvent, got {body:?}"
    );
}
