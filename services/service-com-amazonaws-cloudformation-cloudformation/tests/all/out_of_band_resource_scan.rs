// SPDX-License-Identifier: CC0-1.0

//! A resource scan over resources CloudFormation does NOT manage, end to end over the
//! awsQuery wire.
//!
//! RULE under test: **a resource scan's inventory is the resources this account and Region
//! really holds in the services that own them — not only the ones a stack created — so a
//! resource created out of band is listed by `ListResourceScanResources` under its own type
//! and primary identifier, reporting `ManagedByStack: false`, while a resource a live stack
//! manages is listed once and reports `true`.**
//!
//! Why it matters: a scan exists to find the resources that are NOT under CloudFormation
//! ("the response indicates whether each returned resource is already managed by
//! CloudFormation"), because that is what a generated template is then built from and
//! imported into a stack. A scan that can only see a stack's own resources makes the whole
//! scan → generate → import workflow unusable.
//!
//! The rows differ along what the rule generalises over: the SERVICE the resource lives in
//! and the wire it publishes (Amazon SNS over awsQuery, Amazon DynamoDB over awsJson), and the
//! FORM its primary identifier takes (an ARN, a name). Each out-of-band resource is created
//! through the owning service's own API — never written into this service's store — so a row
//! is listed only because that service really reports holding it.

use std::collections::BTreeMap;

use crate::resource_scan_related_resources::{completed_scan, reported_rows, stack_catalogue};
use crate::stack_lifecycle::{call, deploy, member, router, sns, REGION};
use smithy_server_runtime::MultiServiceRouter;

/// The members a declared or created `AWS::DynamoDB::Table` must carry for DynamoDB to
/// create it: a key schema with the attribute it names defined, and the on-demand capacity
/// mode.
const TABLE_KEY_SCHEMA: &str = r#""KeySchema":[{"AttributeName":"pk","KeyType":"HASH"}],
    "AttributeDefinitions":[{"AttributeName":"pk","AttributeType":"S"}],
    "BillingMode":"PAY_PER_REQUEST""#;

/// One awsJson call to Amazon DynamoDB over the same router: the `X-Amz-Target` names the
/// operation, as a client's SDK sends it.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://dynamodb.{REGION}.amazonaws.com/"))
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/dynamodb/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed DynamoDB request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    (status, answered)
}

/// One resource the scan must surface: the type it is listed under, the primary-identifier
/// key that type is addressed by, and the value the owning service gave it.
#[derive(Debug, Clone)]
struct Expected {
    case: &'static str,
    resource_type: &'static str,
    key: &'static str,
    value: String,
}

/// Create one resource of each kind OUT OF BAND — through the owning service's own API, with
/// no stack anywhere near it — and report what each service named it.
async fn created_out_of_band(router: &MultiServiceRouter) -> Vec<Expected> {
    // Amazon SNS, over awsQuery: a topic is identified by its ARN ("Ref returns the topic
    // ARN").
    let (status, body) = sns(router, "CreateTopic", &[("Name", "out-of-band-topic")]).await;
    assert_eq!(
        status, 200,
        "case \"an SNS topic created out of band\": CreateTopic answered {status} with {body}"
    );
    let topic_arn = crate::harness::xml_text(&body, "TopicArn").unwrap_or_default();
    assert!(
        !topic_arn.is_empty(),
        "case \"an SNS topic created out of band\": expected a TopicArn, got {body}"
    );

    // Amazon DynamoDB, over awsJson: a table is identified by its name.
    let (status, body) = dynamodb(
        router,
        "CreateTable",
        &format!(r#"{{"TableName":"out-of-band-table",{TABLE_KEY_SCHEMA}}}"#),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a DynamoDB table created out of band\": CreateTable answered {status} with \
         {body}"
    );

    vec![
        Expected {
            case: "an SNS topic created out of band",
            resource_type: "AWS::SNS::Topic",
            key: "TopicArn",
            value: topic_arn,
        },
        Expected {
            case: "a DynamoDB table created out of band",
            resource_type: "AWS::DynamoDB::Table",
            key: "TableName",
            value: "out-of-band-table".to_string(),
        },
    ]
}

/// Every row one completed scan lists, as `(type, identifier key, identifier value,
/// ManagedByStack)`.
async fn scanned_rows(
    router: &MultiServiceRouter,
    resource_scan_id: &str,
) -> Vec<(String, String, String, String)> {
    let (status, body) = call(
        router,
        "ListResourceScanResources",
        &[
            member("ResourceScanId", resource_scan_id),
            member("MaxResults", "100"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"reading the scan\": ListResourceScanResources answered {status} with {body}"
    );
    reported_rows("reading the scan", &body, "Resources")
}

#[tokio::test]
async fn a_scan_surfaces_the_resources_no_stack_manages() {
    let router = router();
    let out_of_band = created_out_of_band(&router).await;

    // ...and one resource of the same kind that a STACK manages, so the two halves of the
    // inventory are told apart by what the scan reports about them.
    let managed = r#"{"Resources":{
        "Managed":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"stack-managed-topic"}}}}"#;
    deploy(&router, "scan-managed", managed).await;
    let catalogue: BTreeMap<String, _> = stack_catalogue(&router, "scan-managed").await;
    let managed_topic = catalogue
        .get("Managed")
        .expect("the stack deployed a topic")
        .clone();

    let resource_scan_id = completed_scan(&router).await;
    let rows = scanned_rows(&router, &resource_scan_id).await;

    for expected in &out_of_band {
        let row = rows.iter().find(|(resource_type, _, value, _)| {
            resource_type == expected.resource_type && *value == expected.value
        });
        assert!(
            row.is_some(),
            "case {:?}: expected {} {:?} among the scanned resources, got {rows:?}",
            expected.case,
            expected.resource_type,
            expected.value
        );
        let (_, key, _, managed_by_stack) = row.expect("the row was just found");
        assert_eq!(
            key, expected.key,
            "case {:?}: expected the row to be identified by {:?}, got {key:?}",
            expected.case, expected.key
        );
        assert_eq!(
            managed_by_stack, "false",
            "case {:?}: no stack manages it, so ManagedByStack must be false, got \
             {managed_by_stack:?}",
            expected.case
        );
        // Listed ONCE: one resource is one row.
        let listed = rows
            .iter()
            .filter(|(resource_type, _, value, _)| {
                resource_type == expected.resource_type && *value == expected.value
            })
            .count();
        assert_eq!(
            listed, 1,
            "case {:?}: expected exactly one row for it, got {listed} in {rows:?}",
            expected.case
        );
    }

    // The stack's own topic is in the SAME scan, reported as managed — and once, although
    // Amazon SNS reports holding it too.
    let managed_rows: Vec<&(String, String, String, String)> = rows
        .iter()
        .filter(|(resource_type, _, value, _)| {
            *resource_type == managed_topic.resource_type && *value == managed_topic.value
        })
        .collect();
    assert_eq!(
        managed_rows.len(),
        1,
        "case \"a topic a stack manages\": expected exactly one row for it, got {rows:?}"
    );
    assert_eq!(
        managed_rows[0].3, "true",
        "case \"a topic a stack manages\": expected ManagedByStack true, got {:?}",
        managed_rows[0]
    );
}

/// The scan's own filters reach the out-of-band rows exactly as they reach a stack's: a row
/// is a row whoever created the resource.
#[tokio::test]
async fn the_filters_reach_an_out_of_band_resource() {
    let router = router();
    let out_of_band = created_out_of_band(&router).await;
    let resource_scan_id = completed_scan(&router).await;

    for expected in &out_of_band {
        // By the primary-identifier VALUE the owning service gave it.
        let (status, body) = call(
            &router,
            "ListResourceScanResources",
            &[
                member("ResourceScanId", &resource_scan_id),
                member("ResourceIdentifier", &expected.value),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListResourceScanResources answered {status} with {body}",
            expected.case
        );
        let rows = reported_rows(expected.case, &body, "Resources");
        assert_eq!(
            rows.len(),
            1,
            "case {:?}: expected the identifier filter to select it alone, got {rows:?}",
            expected.case
        );
        assert_eq!(
            (rows[0].0.as_str(), rows[0].2.as_str(), rows[0].3.as_str()),
            (expected.resource_type, expected.value.as_str(), "false"),
            "case {:?}: expected the out-of-band row, got {rows:?}",
            expected.case
        );

        // ...and by the TYPE prefix of the service that owns it.
        let prefix = expected
            .resource_type
            .rsplit_once("::")
            .map(|(namespace, _)| format!("{namespace}::"))
            .unwrap_or_default();
        let (status, body) = call(
            &router,
            "ListResourceScanResources",
            &[
                member("ResourceScanId", &resource_scan_id),
                member("ResourceTypePrefix", &prefix),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListResourceScanResources answered {status} with {body}",
            expected.case
        );
        let rows = reported_rows(expected.case, &body, "Resources");
        assert!(
            rows.iter().any(|(_, _, value, _)| *value == expected.value),
            "case {:?}: expected {prefix:?} to select it, got {rows:?}",
            expected.case
        );
    }
}
