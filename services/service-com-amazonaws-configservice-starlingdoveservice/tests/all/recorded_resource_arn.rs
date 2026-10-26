// SPDX-License-Identifier: CC0-1.0

//! The ARN a configuration item reports for the resource it records.
//!
//! The rule pinned here: every configuration item a read path reports carries the ARN
//! of the resource it records, built from the recorded resource type and resource id —
//! the ARN namespace of the service that OWNS the type as the service field, the Region
//! and the account the item is reported under except where that service's ARNs leave
//! either empty, and the recorded resource id in the resource field behind the
//! qualifier that type's ARNs carry. So `arn` is present on every item of every
//! recorded resource and ends in the id the resource was recorded by, which is how a
//! client resolves a recorded resource to its ARN; a resource id that is ITSELF an ARN
//! is reported verbatim, and a resource type no Amazon Web Services service owns — a
//! CloudFormation registry type recorded through `PutResourceConfig` — carries no ARN,
//! because no AWS service owns such a resource and no call hands one in.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines. The recorded resources are
//! seeded through the store's own write path (`record_discovered_resource`, which the
//! recording pass uses) the way this crate's other suites seed an inventory, because
//! the resource types an AWS service owns are discovered by a recording pass rather
//! than written by a caller — `PutResourceConfig` takes custom resource types only.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};
use std::time::{Duration, SystemTime};

use crate::config_setup_and_rule_loop::{
    recorder_request, with_recorder_and_channel, Emulator, ROLE_ARN,
};

/// The one account every ARN this emulator mints carries.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The Region the harness signs its requests for, and therefore the Region a reported
/// configuration item belongs to.
const HOME_REGION: &str = "us-east-1";

/// Another Region the same store is read from, so a regional ARN can be checked against
/// the Region the item is reported under.
const OTHER_REGION: &str = "eu-west-1";

/// The aggregator the aggregate read paths are reached through.
const AGGREGATOR: &str = "every-region";

/// One recorded resource and the ARN its configuration item must report.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The recorded resource's type. Every AWS type here is a
    /// `com.amazonaws.configservice#ResourceType` value
    /// (`smithy_query(query="shape", shape="ResourceType")`).
    resource_type: &'static str,
    /// The id the resource is recorded by.
    resource_id: &'static str,
    /// The ARN the resource's every configuration item must carry.
    expected: String,
}

/// A row expecting `expected`.
fn row(
    case: &'static str,
    resource_type: &'static str,
    resource_id: &'static str,
    expected: String,
) -> Row {
    Row {
        case,
        resource_type,
        resource_id,
        expected,
    }
}

/// The resources every table seeds, and the ARN each one's configuration item must
/// report.
///
/// Rows differ along what the rule generalises over: the service that owns the type,
/// which scope fields that service's ARNs carry (both, the account only, the Region
/// only, neither), how the resource field spells its qualifier (a `/` qualifier, a `:`
/// qualifier, a path, none at all), whether the ARN namespace is the type's middle
/// segment at all, whether the type's ARN form is one this emulator states or one
/// derived from the type's own segments, and a resource Config records BY its ARN.
fn rows() -> Vec<Row> {
    let account = account();
    vec![
        // A bucket name is unique across every account and Region, so its ARN carries
        // neither — the resource the replayed client resolved to its ARN.
        row(
            "an S3 bucket, whose ARN carries neither Region nor account",
            "AWS::S3::Bucket",
            "recorded-bucket",
            "arn:aws:s3:::recorded-bucket".to_string(),
        ),
        // The same service, a type whose ARN carries both.
        row(
            "an S3 access point, which belongs to one account in one Region",
            "AWS::S3::AccessPoint",
            "reports-ap",
            format!("arn:aws:s3:{HOME_REGION}:{account}:accesspoint/reports-ap"),
        ),
        // IAM is global: the account, no Region.
        row(
            "an IAM role, a global service's resource",
            "AWS::IAM::Role",
            "config-recorder-role",
            format!("arn:aws:iam::{account}:role/config-recorder-role"),
        ),
        row(
            "an IAM instance profile, whose qualifier is hyphenated",
            "AWS::IAM::InstanceProfile",
            "app-profile",
            format!("arn:aws:iam::{account}:instance-profile/app-profile"),
        ),
        // Route 53 is global and its hosted-zone ARNs name no account either.
        row(
            "a Route 53 hosted zone, whose ARN carries neither",
            "AWS::Route53::HostedZone",
            "Z0ABCDEFGHIJKL",
            "arn:aws:route53:::hostedzone/Z0ABCDEFGHIJKL".to_string(),
        ),
        row(
            "a CloudFront distribution, a global service's account-scoped resource",
            "AWS::CloudFront::Distribution",
            "E1XYZABCDEFGHI",
            format!("arn:aws:cloudfront::{account}:distribution/E1XYZABCDEFGHI"),
        ),
        // A colon-separated qualifier.
        row(
            "a Lambda function, whose qualifier is colon-separated",
            "AWS::Lambda::Function",
            "evaluate",
            format!("arn:aws:lambda:{HOME_REGION}:{account}:function:evaluate"),
        ),
        row(
            "a CloudWatch alarm, whose qualifier is colon-separated too",
            "AWS::CloudWatch::Alarm",
            "high-latency",
            format!("arn:aws:cloudwatch:{HOME_REGION}:{account}:alarm:high-latency"),
        ),
        // No qualifier at all.
        row(
            "an SQS queue, whose resource field is the name alone",
            "AWS::SQS::Queue",
            "orders",
            format!("arn:aws:sqs:{HOME_REGION}:{account}:orders"),
        ),
        row(
            "an SNS topic, likewise",
            "AWS::SNS::Topic",
            "alerts",
            format!("arn:aws:sns:{HOME_REGION}:{account}:alerts"),
        ),
        // A namespace the type's middle segment does not spell.
        row(
            "a load balancer, minted in the elasticloadbalancing namespace",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
            "app-lb",
            format!(
                "arn:aws:elasticloadbalancing:{HOME_REGION}:{account}:loadbalancer/app-lb"
            ),
        ),
        row(
            "an OpenSearch domain, minted in the es namespace",
            "AWS::OpenSearch::Domain",
            "search",
            format!("arn:aws:es:{HOME_REGION}:{account}:domain/search"),
        ),
        row(
            "a state machine, minted in the states namespace",
            "AWS::StepFunctions::StateMachine",
            "ingest",
            format!("arn:aws:states:{HOME_REGION}:{account}:stateMachine:ingest"),
        ),
        row(
            "a DB instance, whose qualifier is db",
            "AWS::RDS::DBInstance",
            "orders-db",
            format!("arn:aws:rds:{HOME_REGION}:{account}:db:orders-db"),
        ),
        // A resource field that is a PATH, in an ARN that names the Region but no
        // account.
        row(
            "a REST API, whose ARN names the Region and no account",
            "AWS::ApiGateway::RestApi",
            "a1b2c3d4e5",
            format!("arn:aws:apigateway:{HOME_REGION}::/restapis/a1b2c3d4e5"),
        ),
        row(
            "an HTTP API, whose path qualifier differs from the REST one",
            "AWS::ApiGatewayV2::Api",
            "f6g7h8i9j0",
            format!("arn:aws:apigateway:{HOME_REGION}::/apis/f6g7h8i9j0"),
        ),
        // Types whose form is derived from the type's own segments.
        row(
            "a DynamoDB table, whose form is derived from the type",
            "AWS::DynamoDB::Table",
            "orders",
            format!("arn:aws:dynamodb:{HOME_REGION}:{account}:table/orders"),
        ),
        row(
            "an EC2 instance, likewise",
            "AWS::EC2::Instance",
            "i-0123456789abcdef0",
            format!("arn:aws:ec2:{HOME_REGION}:{account}:instance/i-0123456789abcdef0"),
        ),
        row(
            "an EC2 volume, likewise",
            "AWS::EC2::Volume",
            "vol-0001",
            format!("arn:aws:ec2:{HOME_REGION}:{account}:volume/vol-0001"),
        ),
        row(
            "a security group, whose derived qualifier is hyphenated",
            "AWS::EC2::SecurityGroup",
            "sg-0001",
            format!("arn:aws:ec2:{HOME_REGION}:{account}:security-group/sg-0001"),
        ),
        row(
            "a VPC, whose derived qualifier is one acronym",
            "AWS::EC2::VPC",
            "vpc-0001",
            format!("arn:aws:ec2:{HOME_REGION}:{account}:vpc/vpc-0001"),
        ),
        row(
            "a VPC endpoint, whose derived qualifier splits after the acronym",
            "AWS::EC2::VPCEndpoint",
            "vpce-0001",
            format!("arn:aws:ec2:{HOME_REGION}:{account}:vpc-endpoint/vpce-0001"),
        ),
        row(
            "an ECR repository, likewise derived",
            "AWS::ECR::Repository",
            "service-images",
            format!("arn:aws:ecr:{HOME_REGION}:{account}:repository/service-images"),
        ),
        row(
            "a KMS key, likewise derived",
            "AWS::KMS::Key",
            "1234abcd-12ab-34cd-56ef-1234567890ab",
            format!(
                "arn:aws:kms:{HOME_REGION}:{account}:key/1234abcd-12ab-34cd-56ef-1234567890ab"
            ),
        ),
        // A resource Config records BY its ARN: that id is the ARN, and no second one is
        // derived from it.
        row(
            "a resource recorded by its own ARN",
            "AWS::ECS::TaskDefinition",
            "arn:aws:ecs:us-east-1:000000000000:task-definition/app:7",
            "arn:aws:ecs:us-east-1:000000000000:task-definition/app:7".to_string(),
        ),
    ]
}

/// The custom resource types the custom-type case records, each a CloudFormation
/// registry type name (`Organization::Service::Resource`) whose organization is none of
/// the ones `PutResourceConfig` reserves.
const CUSTOM_RESOURCES: [(&str, &str); 3] = [
    ("MyOrg::Payments::Ledger", "ledger-1"),
    ("Contoso::Billing::Invoice", "invoice-7"),
    ("Example::Fleet::Vehicle", "vehicle-42"),
];

/// The instant a seeded configuration item was captured: a fixed point in the past, so
/// every read falls inside the default time range.
fn captured_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Record a configuration item for `resource_type`/`resource_id` through the store's own
/// write path, carrying `configuration` and captured at `captured`.
fn record(
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
    configuration: &str,
    captured: SystemTime,
) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    store.record_discovered_resource(DiscoveredResourceEntry {
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
        resource_name: Some(resource_id.to_string()),
        configuration: Some(configuration.to_string()),
        first_recorded_at: Some(captured),
        last_recorded_at: Some(captured),
        ..DiscoveredResourceEntry::default()
    });
}

/// An emulator with a customer managed configuration recorder (which a
/// configuration-item read needs), holding one recorded configuration item per row.
async fn emulator_recording(rows: &[Row]) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    for row in rows {
        record(
            &emulator,
            row.resource_type,
            row.resource_id,
            r#"{"state":"recorded"}"#,
            captured_at(),
        );
    }
    emulator
}

/// Every configuration item `GetResourceConfigHistory` reports for one resource.
async fn history(emulator: &Emulator, resource_type: &str, resource_id: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "GetResourceConfigHistory",
            &json!({ "resourceType": resource_type, "resourceId": resource_id }),
        )
        .await;
    body["configurationItems"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("{resource_type}/{resource_id}: expected configurationItems, got {body}")
        })
        .clone()
}

/// The `arn` of a reported configuration item, absent when the item carries none.
fn reported_arn(item: &Value) -> Option<&str> {
    item.get("arn").and_then(Value::as_str)
}

/// The documents a `SelectResourceConfig` answer carries, parsed back out of the JSON
/// each result is.
fn selected(case: &str, body: &Value) -> Vec<Value> {
    body["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Results, got {body}"))
        .iter()
        .map(|result| {
            let text = result.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a result is a JSON document, got {body}")
            });
            serde_json::from_str::<Value>(text)
                .unwrap_or_else(|_| panic!("case {case:?}: a result parses as JSON, got {text:?}"))
        })
        .collect()
}

#[tokio::test]
async fn every_configuration_item_reports_the_arn_of_the_recorded_resource() {
    let rows = rows();
    let emulator = emulator_recording(&rows).await;

    for row in &rows {
        let items = history(&emulator, row.resource_type, row.resource_id).await;
        assert_eq!(
            items.len(),
            1,
            "case {:?}: one recorded state is one configuration item, got {items:?}",
            row.case
        );
        let item = &items[items.len() - 1];
        assert_eq!(
            reported_arn(item),
            Some(row.expected.as_str()),
            "case {:?}: expected arn {:?}, got {item}",
            row.case,
            row.expected
        );
        // The invariant a client relies on to resolve a recorded resource: the ARN
        // reported for it names that very resource.
        assert!(
            row.expected.ends_with(row.resource_id),
            "case {:?}: the reported ARN must end in the recorded resource id {:?}, got {:?}",
            row.case,
            row.resource_id,
            row.expected
        );

        // The read is a report: a second one must see exactly the same item.
        let again = history(&emulator, row.resource_type, row.resource_id).await;
        assert_eq!(
            again, items,
            "case {:?}: a second read must report the same configuration items",
            row.case
        );
    }
}

#[tokio::test]
async fn every_item_of_a_resource_with_a_history_carries_the_same_arn() {
    let rows = rows();
    let emulator = emulator_recording(&rows).await;

    // Rows differ along the ARN form of the resource whose history is walked: one whose
    // ARN carries neither scope field, one carrying the account only, and one carrying
    // both.
    let walked: [(&str, &str, &str); 3] = [
        ("a bucket", "AWS::S3::Bucket", "recorded-bucket"),
        ("a role", "AWS::IAM::Role", "config-recorder-role"),
        ("a table", "AWS::DynamoDB::Table", "orders"),
    ];
    let expected: Vec<String> = walked
        .iter()
        .map(|(_, resource_type, resource_id)| {
            rows.iter()
                .find(|row| row.resource_type == *resource_type && row.resource_id == *resource_id)
                .map(|row| row.expected.clone())
                .expect("the walked resources are rows of the table")
        })
        .collect();

    // Two further states of each resource, so each one has a three-item history.
    for (index, (_, resource_type, resource_id)) in walked.iter().enumerate() {
        for state in 1..=2 {
            record(
                &emulator,
                resource_type,
                resource_id,
                &format!(r#"{{"state":"changed-{state}"}}"#),
                captured_at() + Duration::from_secs(60 * (index + state) as u64),
            );
        }
    }

    for ((case, resource_type, resource_id), expected) in walked.iter().zip(expected) {
        let items = history(&emulator, resource_type, resource_id).await;
        assert_eq!(
            items.len(),
            3,
            "case {case:?}: three recorded states are three configuration items, got \
             {items:?}"
        );
        for item in &items {
            assert_eq!(
                reported_arn(item),
                Some(expected.as_str()),
                "case {case:?}: every item of a resource's history reports its ARN, got \
                 {item}"
            );
        }
    }
}

#[tokio::test]
async fn a_regional_arn_names_the_region_the_item_is_reported_under() {
    let rows = rows();
    let emulator = emulator_recording(&rows).await;
    // The same store, read by a caller signing for another Region: a configuration item
    // belongs to the Region it is reported under, and so does the ARN of a resource
    // whose service scopes its ARNs by Region.
    let elsewhere = emulator.in_other_region(OTHER_REGION);

    for row in &rows {
        let items = history(&elsewhere, row.resource_type, row.resource_id).await;
        let item = &items[0];
        // A resource recorded BY its ARN reports that ARN whichever Region reads it:
        // the id is the ARN, and nothing about it is derived from the reading Region.
        let expected = if row.resource_id.starts_with("arn:") {
            row.expected.clone()
        } else {
            row.expected.replace(HOME_REGION, OTHER_REGION)
        };
        assert_eq!(
            reported_arn(item),
            Some(expected.as_str()),
            "case {:?}: expected arn {expected:?} when the item is reported in \
             {OTHER_REGION}, got {item}",
            row.case
        );

        // A DERIVED ARN's Region field is the Region the item itself reports, or empty
        // for a service whose ARNs carry no Region.
        if !row.resource_id.starts_with("arn:") {
            let reported_region = item["awsRegion"].as_str().unwrap_or_default();
            let arn_region = reported_arn(item)
                .and_then(|arn| arn.split(':').nth(3))
                .unwrap_or_default();
            assert!(
                arn_region.is_empty() || arn_region == reported_region,
                "case {:?}: expected the ARN's Region field to be empty or \
                 {reported_region:?}, got {arn_region:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn the_aggregate_reads_report_the_same_arn_as_the_resource_history() {
    let rows = rows();
    let emulator = emulator_recording(&rows).await;
    emulator
        .ok(
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "AccountAggregationSources": [{
                    "AccountIds": [account()],
                    "AllAwsRegions": true,
                }],
            }),
        )
        .await;

    for row in &rows {
        let identifier = json!({
            "SourceAccountId": account(),
            "SourceRegion": HOME_REGION,
            "ResourceType": row.resource_type,
            "ResourceId": row.resource_id,
        });
        let single = emulator
            .ok(
                "GetAggregateResourceConfig",
                &json!({
                    "ConfigurationAggregatorName": AGGREGATOR,
                    "ResourceIdentifier": identifier,
                }),
            )
            .await;
        assert_eq!(
            reported_arn(&single["ConfigurationItem"]),
            Some(row.expected.as_str()),
            "case {:?}: the single aggregate read must report the same arn as the \
             resource history, got {single}",
            row.case
        );

        let batch = emulator
            .ok(
                "BatchGetAggregateResourceConfig",
                &json!({
                    "ConfigurationAggregatorName": AGGREGATOR,
                    "ResourceIdentifiers": [identifier],
                }),
            )
            .await;
        let items = batch["BaseConfigurationItems"]
            .as_array()
            .unwrap_or_else(|| panic!("expected BaseConfigurationItems, got {batch}"));
        assert_eq!(
            items.len(),
            1,
            "case {:?}: the batch read must report the one identifier's item, got {batch}",
            row.case
        );
        assert_eq!(
            reported_arn(&items[0]),
            Some(row.expected.as_str()),
            "case {:?}: the batch aggregate read must report the same arn, got {batch}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_custom_resource_type_no_aws_service_owns_carries_no_arn() {
    // A custom resource type is read through the Config query language: the modelled
    // `ResourceType` enum the other read paths take names Amazon Web Services types
    // only, while `PutResourceConfig` records custom ones.
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    // `PutResourceConfig` records a resource's configuration only while a recorder is
    // RECORDING; a recorder stored with no name of its own is named `default`.
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;
    for (resource_type, resource_id) in CUSTOM_RESOURCES {
        emulator
            .ok(
                "PutResourceConfig",
                &json!({
                    "ResourceType": resource_type,
                    "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
                    "ResourceId": resource_id,
                    "ResourceName": resource_id,
                    "Configuration": r#"{"state":"recorded"}"#,
                }),
            )
            .await;
    }
    // One resource of a type an AWS service DOES own, recorded beside them, so the same
    // query shows the difference rather than the query reporting nothing at all.
    record(
        &emulator,
        "AWS::S3::Bucket",
        "recorded-bucket",
        r#"{"state":"recorded"}"#,
        captured_at(),
    );

    for (resource_type, resource_id) in CUSTOM_RESOURCES {
        let body = emulator
            .ok(
                "SelectResourceConfig",
                &json!({
                    "Expression":
                        format!("SELECT arn, resourceId WHERE resourceType = '{resource_type}'"),
                }),
            )
            .await;
        let documents = selected(resource_type, &body);
        assert_eq!(
            documents.len(),
            1,
            "case {resource_type:?}: the recorded resource must be selected, got {body}"
        );
        assert_eq!(
            documents[0]["resourceId"],
            json!(resource_id),
            "case {resource_type:?}: expected the recorded resource id, got {}",
            documents[0]
        );
        assert!(
            documents[0].get("arn").is_none(),
            "case {resource_type:?}: a resource type no AWS service owns has no ARN to \
             report, got {}",
            documents[0]
        );
    }

    let body = emulator
        .ok(
            "SelectResourceConfig",
            &json!({ "Expression": "SELECT arn, resourceId WHERE resourceType = 'AWS::S3::Bucket'" }),
        )
        .await;
    let documents = selected("an AWS resource type", &body);
    assert_eq!(
        documents.len(),
        1,
        "the bucket must be selected, got {body}"
    );
    assert_eq!(
        documents[0]["arn"],
        json!("arn:aws:s3:::recorded-bucket"),
        "the same query reports the ARN of a resource an AWS service owns, got {}",
        documents[0]
    );
}
