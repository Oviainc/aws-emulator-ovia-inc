// SPDX-License-Identifier: CC0-1.0

//! What a resource scan FINDS: the account's own inventory of a Region, end to end over the
//! awsQuery wire.
//!
//! The rule every case here holds the service to: **a resource scan is an inventory of the
//! resources an account holds in a Region, so a resource created straight through its owning
//! service's API — with no CloudFormation stack involved — is one of the resources the scan
//! lists, identified by its type's primary identifier and reported `ManagedByStack` false;
//! the same resource deployed BY a stack is reported once, with `ManagedByStack` true.**
//!
//! The table spans the class that rule generalises over rather than the one recorded call: a
//! resource whose primary identifier is an ARN its owning service minted (`AWS::SNS::Topic`),
//! one identified by a url (`AWS::SQS::Queue`) and one identified by its own name
//! (`AWS::DynamoDB::Table`) — three types, in three services, over two wire protocols. Each
//! row is created through that service's own API exactly as a client creates it, and every
//! value asserted is one a service answered: the identifier comes from the create, the rows
//! from `ListResourceScanResources` and the stack's own row from `ListStackResources`.
//!
//! The cases beyond the table are the other halves of the same rule: a scan's `ScanFilters`
//! and a read's filters narrow the discovered rows exactly as they narrow a stack's, the tags
//! those filters select on are the ones the OWNING service holds, and a resource of another
//! Region is outside this Region's inventory.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{
    create_generated_template, describe_generated_template, wait_for_the_generation,
};
use crate::harness::{call, member, xml_text, xml_texts, REGION};
use crate::resource_scan_related_resources::{completed_scan, reported_rows, stack_catalogue};
use crate::stack_lifecycle::{deploy, sns, sns_in};

/// A router carrying CloudFormation and the services this suite creates resources in, all on
/// ONE engine — the shape `core/router/src/lib.rs` mounts every service in.
///
/// Its own router rather than the shared harness's: the services a scan enumerates are not
/// the services a stack's deployment needs, and mounting one of them for every suite would
/// change what the accounts those suites build hold.
fn inventory_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// A second Region, for the case that holds a scan to the Region it was signed for.
const OTHER_REGION: &str = "us-west-2";

/// The tag every resource this suite creates out of band carries, so a read's `TagKey` and
/// `TagValue` filters have something to select a DISCOVERED row by — read from the service
/// that holds the resource, there being no stack to have propagated it.
const TAG: (&str, &str) = ("Tier", "gold");

/// The awsJson version Amazon DynamoDB declares (`awsJson1_0`).
const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";

/// The service shape an awsJson `X-Amz-Target` names an operation under
/// (`DynamoDB_20120810`).
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// One awsJson call to another service over the same router, signed the way a client's SDK
/// signs it: the status and the parsed answer document.
///
/// `target` is the operation under the called service's own shape.
async fn aws_json(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("content-type", AWS_JSON_1_0)
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed awsJson request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, document)
}

/// Which service holds the resource one row of the table is about, and therefore which API
/// creates it and which handle that API answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Amazon SNS's `CreateTopic`.
    Topic,
    /// Amazon DynamoDB's `CreateTable`.
    Table,
}

/// One row of the table: a resource type, the API that creates one of its resources, and the
/// primary-identifier property a scan must report that resource under.
struct Row {
    case: &'static str,
    kind: Kind,
    /// The CloudFormation resource type whose resources that service holds.
    resource_type: &'static str,
    /// The property a scan's row carries the identifier under.
    ///
    /// Source: each type's documented "Return values" — "Ref returns the topic ARN" for
    /// `AWS::SNS::Topic`, "Ref returns the table name" for `AWS::DynamoDB::Table`. Written
    /// out per row rather than read from the service under test, so a row asserts the
    /// published property and not the emulator's own idea of it.
    identifier_key: &'static str,
    /// The name the out-of-band create asks for.
    name: &'static str,
}

/// The table: one row per service this suite creates a resource in out of band. Both of the
/// catalogued types whose service is mounted here are covered; the third catalogued type
/// (`AWS::SQS::Queue`) is covered by the module's own unit tests, its owning service not
/// being one this crate's test target mounts.
const ROWS: &[Row] = &[
    Row {
        case: "a topic created straight through Amazon SNS",
        kind: Kind::Topic,
        resource_type: "AWS::SNS::Topic",
        identifier_key: "TopicArn",
        name: "inventory-unmanaged-topic",
    },
    Row {
        case: "a table created straight through Amazon DynamoDB",
        kind: Kind::Table,
        resource_type: "AWS::DynamoDB::Table",
        identifier_key: "TableName",
        name: "inventory-unmanaged-table",
    },
];

/// Create one resource through the API of the service that owns it, with no stack involved,
/// and answer with the handle that service identifies it by — a topic's ARN, a table's name.
async fn create_out_of_band(router: &MultiServiceRouter, row: &Row) -> String {
    match row.kind {
        Kind::Topic => {
            let (status, body) = sns(
                router,
                "CreateTopic",
                &[
                    ("Name", row.name),
                    ("Tags.member.1.Key", TAG.0),
                    ("Tags.member.1.Value", TAG.1),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: CreateTopic answered {status} with {body}",
                row.case
            );
            xml_text(&body, "TopicArn").unwrap_or_else(|| {
                panic!("case {:?}: expected a TopicArn, got {body}", row.case)
            })
        }
        Kind::Table => {
            let (status, document) = aws_json(
                router,
                "dynamodb",
                &format!("{DYNAMODB_TARGET_PREFIX}.CreateTable"),
                &json!({
                    "TableName": row.name,
                    "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                    "BillingMode": "PAY_PER_REQUEST",
                    "Tags": [{"Key": TAG.0, "Value": TAG.1}],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: CreateTable answered {status} with {document}",
                row.case
            );
            document
                .get("TableDescription")
                .and_then(|table| table.get("TableName"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a TableName, got {document}",
                        row.case
                    )
                })
                .to_string()
        }
    }
}

/// One row as `ListResourceScanResources` reports it: the resource type, the key and value of
/// the primary identifier it is addressed by, and what it says about being managed by a stack
/// — the tuple the shared reader answers ([`reported_rows`]).
type ReportedRow = (String, String, String, String);

/// One row of the filter table: the case, the request members its filters are, and the rows
/// the read must come back with.
type FilteredRead = (&'static str, Vec<(String, String)>, Vec<ReportedRow>);

/// `ListResourceScanResources` over one scan, with whatever else the case names.
async fn list_resources(
    router: &MultiServiceRouter,
    resource_scan_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("ResourceScanId", resource_scan_id)];
    members.extend_from_slice(extra);
    call(router, "ListResourceScanResources", &members).await
}

/// The rows one read answered, sorted, each as its type, its primary-identifier pair and what
/// it says about being managed by a stack.
async fn scanned(
    case: &str,
    router: &MultiServiceRouter,
    resource_scan_id: &str,
    extra: &[(String, String)],
) -> Vec<ReportedRow> {
    let (status, body) = list_resources(router, resource_scan_id, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListResourceScanResources answered {status} with {body}"
    );
    reported_rows(case, &body, "Resources")
}

/// The row a resource the account holds outside CloudFormation must come back as.
fn unmanaged_row(row: &Row, identifier: &str) -> ReportedRow {
    (
        row.resource_type.to_string(),
        row.identifier_key.to_string(),
        identifier.to_string(),
        "false".to_string(),
    )
}

/// `StartResourceScan` with one `ScanFilters` entry naming `types`, in the awsQuery flattened
/// form; naming none sends no filter at all. `ScanFilters` is `@length(min: 1, max: 1)`, so
/// there is never more than one filter.
async fn start_scan(router: &MultiServiceRouter, types: &[&str]) -> String {
    let members: Vec<(String, String)> = types
        .iter()
        .enumerate()
        .map(|(position, value)| {
            member(
                &format!("ScanFilters.member.1.Types.member.{}", position + 1),
                value,
            )
        })
        .collect();
    let (status, body) = call(router, "StartResourceScan", &members).await;
    assert_eq!(
        status, 200,
        "case \"starting the scan\": StartResourceScan answered {status} with {body}"
    );
    let resource_scan_id = xml_text(&body, "ResourceScanId").unwrap_or_else(|| {
        panic!("case \"starting the scan\": expected a ResourceScanId, got {body}")
    });
    tokio::time::sleep(scans::SCAN_DELAY + std::time::Duration::from_millis(250)).await;
    resource_scan_id
}

#[tokio::test]
async fn a_resource_created_outside_cloudformation_is_discovered_unmanaged() {
    let router = inventory_router();

    // Every row's resource created through its OWN service's API: no stack exists in this
    // account at all.
    let mut identifiers: BTreeMap<&str, String> = BTreeMap::new();
    for row in ROWS {
        identifiers.insert(row.case, create_out_of_band(&router, row).await);
    }

    let resource_scan_id = completed_scan(&router).await;

    // Narrowed to its own type, each row is the one resource of that type the account holds,
    // addressed by its published primary identifier and reported as managed by no stack.
    for row in ROWS {
        let case = row.case;
        let identifier = &identifiers[case];
        let found = scanned(
            case,
            &router,
            &resource_scan_id,
            &[member("ResourceTypePrefix", row.resource_type)],
        )
        .await;
        assert_eq!(
            found,
            vec![unmanaged_row(row, identifier)],
            "case {case:?}: expected the resource the account holds reported unmanaged, got \
             {found:?}"
        );
    }

    // And the unnarrowed read reports all of them, so the inventory is one set rather than
    // something a type filter conjures.
    let case = "the whole inventory";
    let everything = scanned(case, &router, &resource_scan_id, &[]).await;
    let mut expected: Vec<ReportedRow> = ROWS
        .iter()
        .map(|row| unmanaged_row(row, &identifiers[row.case]))
        .collect();
    expected.sort();
    assert_eq!(
        everything, expected,
        "case {case:?}: expected every resource of the account, got {everything:?}"
    );

    // The scan's own record agrees: `DescribeResourceScan` reports as many resources as the
    // read did, and names each type among them.
    let (status, body) = call(
        &router,
        "DescribeResourceScan",
        &[member("ResourceScanId", &resource_scan_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeResourceScan answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourcesScanned"),
        Some(ROWS.len().to_string()),
        "case {case:?}: expected the scan to have listed every resource, got {body}"
    );
    for row in ROWS {
        assert!(
            body.contains(row.resource_type),
            "case {:?}: expected {} among the scan's resource types, got {body}",
            row.case,
            row.resource_type
        );
    }
}

#[tokio::test]
async fn a_stack_managed_resource_is_reported_managed_and_exactly_once() {
    let router = inventory_router();

    // One topic a STACK created, and one created straight through Amazon SNS: the same type,
    // in the same account and Region, differing only in whether CloudFormation manages it.
    deploy(
        &router,
        "inventory-managed",
        r#"{"Resources":{"Alpha":{"Type":"AWS::SNS::Topic",
            "Properties":{"TopicName":"inventory-managed-topic"}}}}"#,
    )
    .await;
    let managed = stack_catalogue(&router, "inventory-managed").await;
    let managed = managed
        .get("Alpha")
        .expect("the stack's own topic, as ListStackResources reports it")
        .clone();
    let unmanaged = create_out_of_band(&router, &ROWS[0]).await;

    let resource_scan_id = completed_scan(&router).await;
    let case = "one managed topic and one unmanaged topic";
    let found = scanned(
        case,
        &router,
        &resource_scan_id,
        &[member("ResourceTypePrefix", "AWS::SNS::Topic")],
    )
    .await;

    let mut expected = vec![
        (
            managed.resource_type.clone(),
            managed.key.clone(),
            managed.value.clone(),
            "true".to_string(),
        ),
        unmanaged_row(&ROWS[0], &unmanaged),
    ];
    expected.sort();
    assert_eq!(
        found, expected,
        "case {case:?}: expected the stack's topic reported managed and the other one not, \
         each exactly once, got {found:?}"
    );
}

#[tokio::test]
async fn the_filters_narrow_the_accounts_own_resources_too() {
    let router = inventory_router();
    let mut identifiers: BTreeMap<&str, String> = BTreeMap::new();
    for row in ROWS {
        identifiers.insert(row.case, create_out_of_band(&router, row).await);
    }

    // A scan whose `ScanFilters` names one type lists the resources of THAT type and no
    // other, whether a stack manages them or not.
    let topic = &ROWS[0];
    let case = "a scan filtered to one resource type";
    let filtered = start_scan(&router, &[topic.resource_type]).await;
    let found = scanned(case, &router, &filtered, &[]).await;
    assert_eq!(
        found,
        vec![unmanaged_row(topic, &identifiers[topic.case])],
        "case {case:?}: expected only the filtered type's resource, got {found:?}"
    );

    // And a read's own filters select over the same rows: the tags they select on are the
    // ones the OWNING service holds for the resource, there being no stack to propagate any.
    let resource_scan_id = start_scan(&router, &[]).await;
    let mut all: Vec<ReportedRow> = ROWS
        .iter()
        .map(|row| unmanaged_row(row, &identifiers[row.case]))
        .collect();
    all.sort();
    let reads: Vec<FilteredRead> = vec![
        (
            "TagKey: the tag each resource's own create asked for",
            vec![member("TagKey", TAG.0)],
            all.clone(),
        ),
        (
            "TagValue: that tag's value",
            vec![member("TagValue", TAG.1)],
            all.clone(),
        ),
        (
            "TagKey: a key nothing in the account carries",
            vec![member("TagKey", "NoSuchTag")],
            Vec::new(),
        ),
        (
            "ResourceIdentifier: one resource's own identifier value",
            vec![member("ResourceIdentifier", &identifiers[topic.case])],
            vec![unmanaged_row(topic, &identifiers[topic.case])],
        ),
        (
            "ResourceTypePrefix: the namespace every type is under",
            vec![member("ResourceTypePrefix", "AWS::")],
            all.clone(),
        ),
        (
            "ResourceTypePrefix: a namespace the account holds nothing of",
            vec![member("ResourceTypePrefix", "AWS::Kinesis::")],
            Vec::new(),
        ),
    ];
    for (case, filters, expected) in reads {
        let found = scanned(case, &router, &resource_scan_id, &filters).await;
        assert_eq!(
            found, expected,
            "case {case:?}: expected the filtered rows, got {found:?}"
        );
    }
}

#[tokio::test]
async fn a_generated_templates_refresh_reads_the_accounts_own_resources_too() {
    let router = inventory_router();
    let topic = &ROWS[0];
    let topic_arn = create_out_of_band(&router, topic).await;
    let case = "a refresh over a resource no stack manages";

    // A generated template covering that resource, identified under a property that is NOT
    // the one its type publishes: "update the resource properties in the generated template
    // with their current live state" is what replaces it with the identifier the resource is
    // really addressed by — which can only be read from the account's own inventory, no stack
    // managing this topic.
    let name = "inventory-refreshed-template";
    let (status, body) = create_generated_template(
        &router,
        name,
        &[
            member("Resources.member.1.ResourceType", topic.resource_type),
            member("Resources.member.1.LogicalResourceId", "ImportedTopic"),
            member("Resources.member.1.ResourceIdentifier.entry.1.key", "Arn"),
            member(
                "Resources.member.1.ResourceIdentifier.entry.1.value",
                &topic_arn,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
    );
    wait_for_the_generation().await;
    let (status, body) = describe_generated_template(&router, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
    );
    assert_eq!(
        xml_texts(&body, "key"),
        vec!["Arn".to_string()],
        "case {case:?}: expected the record to carry the identifier the request named, got \
         {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateGeneratedTemplate",
        &[
            member("GeneratedTemplateName", name),
            member("RefreshAllResources", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateGeneratedTemplate answered {status} with {body}"
    );
    wait_for_the_generation().await;
    let (status, body) = describe_generated_template(&router, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
    );
    assert_eq!(
        (xml_texts(&body, "key"), xml_texts(&body, "value")),
        (
            vec![topic.identifier_key.to_string()],
            vec![topic_arn.clone()]
        ),
        "case {case:?}: expected the live identifier of the resource the account holds, got \
         {body}"
    );
}

#[tokio::test]
async fn a_resource_of_another_region_is_outside_this_regions_inventory() {
    let router = inventory_router();

    // The same kind of resource in two Regions, each created through its owning service's own
    // regional endpoint.
    let here = create_out_of_band(&router, &ROWS[0]).await;
    let case = "a topic another Region holds";
    let (status, body) = sns_in(
        &router,
        OTHER_REGION,
        "CreateTopic",
        &[("Name", "inventory-other-region-topic")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic answered {status} with {body}"
    );
    let elsewhere = xml_text(&body, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: expected a TopicArn, got {body}"));

    let resource_scan_id = completed_scan(&router).await;
    let found = scanned(case, &router, &resource_scan_id, &[]).await;
    assert_eq!(
        found,
        vec![unmanaged_row(&ROWS[0], &here)],
        "case {case:?}: expected a scan of one Region to report only what that Region holds, \
         got {found:?}"
    );
    assert!(
        !found.iter().any(|(_, _, value, _)| value == &elsewhere),
        "case {case:?}: expected {elsewhere} outside this Region's scan, got {found:?}"
    );
}
