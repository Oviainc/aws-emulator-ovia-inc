// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource-type catalog this service accepts.
//!
//! The rule under test: a resource type this API names is accepted only when it is one
//! of the types the readiness feature can audit — the closed catalog the
//! `ResourceSetType` member's documentation enumerates — so every member that carries a
//! resource type (the type a resource set declares, the type the rule listing filters
//! by) accepts exactly that catalog and refuses anything else with a 400
//! `ValidationException`, whatever shape the rejected value has. Satisfying the member's
//! `@pattern` is not enough: a pattern-shaped type outside the catalog names no type
//! this service has readiness rules for.
//!
//! Every assertion goes over the wire, through the same router the production emulator
//! mounts: only the HTTP status, the `x-amzn-errortype` header and the JSON body are
//! inspected, so nothing here can pass on a value that was not computed from persisted
//! state.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which
//! catalog entry is used (every one of the twenty, including entries the issue did not
//! cite), and the FORM of a pattern-valid value outside the catalog — a real AWS type
//! the feature does not audit, a wholly invented one, a catalog entry with its casing
//! changed, a near-miss plural, and a very long segment. The `AWS::S3::Bucket` value the
//! issue recorded is one row.

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::SUPPORTED_RESOURCE_TYPES;

// The harness (wire-only `Reply`, in-process router `Emulator`) is shared with the
// lifecycle suite rather than copied.
use crate::readiness_check_lifecycle::Emulator;

/// The resource types the `ResourceSetType` member's own documentation enumerates
/// (model: `smithy_query(query="doc", shape="CreateResourceSetRequest",
/// member="ResourceSetType")`). Written out here rather than read from the crate's
/// [`SUPPORTED_RESOURCE_TYPES`], so the table pins the documented values themselves; the
/// first test asserts the two agree. The issue cited none of these as accepted values.
const DOCUMENTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApiGateway::Stage",
    "AWS::ApiGatewayV2::Stage",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::CloudWatch::Alarm",
    "AWS::EC2::CustomerGateway",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::MSK::Cluster",
    "AWS::RDS::DBCluster",
    "AWS::Route53::HealthCheck",
    "AWS::SQS::Queue",
    "AWS::SNS::Topic",
    "AWS::SNS::Subscription",
    "AWS::EC2::VPC",
    "AWS::EC2::VPNConnection",
    "AWS::EC2::VPNGateway",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
];

/// Values that satisfy `ResourceSetType`'s `^AWS::[A-Za-z0-9]+::[A-Za-z0-9]+$` pattern
/// (model: `smithy_query(query="doc", shape="CreateResourceSetRequest",
/// member="ResourceSetType")`) yet name no type the catalog above holds. Each row is a
/// different way of being outside the catalog while inside the pattern.
fn unsupported_but_pattern_valid() -> Vec<(&'static str, &'static str)> {
    vec![
        // The value the issue recorded: a real AWS type the readiness feature does not
        // audit.
        ("a real AWS type outside the catalog", "AWS::S3::Bucket"),
        ("another real AWS type", "AWS::EC2::Instance"),
        ("a third real AWS type", "AWS::Kinesis::Stream"),
        ("an invented service and type", "AWS::Foo::Bar"),
        // A catalog entry with its casing changed: still pattern-valid, still not the
        // documented value.
        ("a catalog entry mis-cased", "AWS::Dynamodb::Table"),
        ("a catalog entry pluralised", "AWS::SQS::Queues"),
        (
            "a long pattern-valid segment",
            "AWS::Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa::Bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ),
    ]
}

/// A pattern-valid value is rejected by the shared catalog, not by this suite's table:
/// a row that the crate secretly supports would make the refusal assertions vacuous.
#[test]
fn the_crate_catalog_is_exactly_the_documented_catalog() {
    let mut documented: Vec<&str> = DOCUMENTED_RESOURCE_TYPES.to_vec();
    documented.sort_unstable();
    let mut supported: Vec<&str> = SUPPORTED_RESOURCE_TYPES.to_vec();
    supported.sort_unstable();
    assert_eq!(
        supported, documented,
        "case {:?}: expected the crate catalog to be the documented one, got {supported:?}",
        "catalog agreement"
    );

    for (case, value) in unsupported_but_pattern_valid() {
        assert!(
            !SUPPORTED_RESOURCE_TYPES.contains(&value),
            "case {case:?}: {value:?} must be outside the catalog for this table to mean anything"
        );
    }
}

#[tokio::test]
async fn every_documented_resource_type_is_accepted_and_read_back() {
    let emulator = Emulator::new();

    for (index, resource_set_type) in DOCUMENTED_RESOURCE_TYPES.iter().enumerate() {
        let name = format!("catalog_set_{index}");
        let created = emulator
            .send(
                "POST",
                "/resourcesets",
                Some(json!({
                    "resourceSetName": name,
                    "resourceSetType": resource_set_type,
                    "resources": [],
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {resource_set_type:?}: expected HTTP 200, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("resourceSetType").as_deref(),
            Some(*resource_set_type),
            "case {resource_set_type:?}: expected the submitted type back, got {:?}",
            created.string("resourceSetType")
        );

        let fetched = emulator
            .send("GET", &format!("/resourcesets/{name}"), None)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {resource_set_type:?}: expected the record to be readable, got {} / {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("resourceSetType").as_deref(),
            Some(*resource_set_type),
            "case {resource_set_type:?}: expected the stored type read back, got {:?}",
            fetched.string("resourceSetType")
        );
    }
}

#[tokio::test]
async fn a_pattern_valid_type_outside_the_catalog_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    for (index, (case, resource_set_type)) in unsupported_but_pattern_valid()
        .into_iter()
        .enumerate()
    {
        let name = format!("unsupported_type_{index}");
        let created = emulator
            .send(
                "POST",
                "/resourcesets",
                Some(json!({
                    "resourceSetName": name,
                    "resourceSetType": resource_set_type,
                    "resources": [{"resourceArn": "arn:aws:s3:::bucket-1"}],
                })),
            )
            .await;
        assert_eq!(
            created.status, 400,
            "case {case:?} ({resource_set_type:?}): expected HTTP 400, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "ValidationException",
            "case {case:?} ({resource_set_type:?}): expected ValidationException, got {:?} / {}",
            created.error_code(),
            created.body
        );

        let fetched = emulator
            .send("GET", &format!("/resourcesets/{name}"), None)
            .await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?} ({resource_set_type:?}): a refused create must store nothing, got {} / {}",
            fetched.status, fetched.body
        );

        let listed = emulator.send("GET", "/resourcesets", None).await;
        let names: Vec<String> = listed
            .field("resourceSets")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("resourceSetName").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !names.contains(&name),
            "case {case:?} ({resource_set_type:?}): a refused create must not be listed, got {names:?}"
        );
    }
}

#[tokio::test]
async fn the_create_and_rule_surfaces_agree_on_which_types_exist() {
    let emulator = Emulator::new();

    // A type in the catalog: both surfaces accept it.
    for (index, resource_type) in DOCUMENTED_RESOURCE_TYPES.iter().enumerate() {
        let rules = emulator
            .send("GET", &format!("/rules?resourceType={resource_type}"), None)
            .await;
        let created = emulator
            .send(
                "POST",
                "/resourcesets",
                Some(json!({
                    "resourceSetName": format!("agree_set_{index}"),
                    "resourceSetType": resource_type,
                    "resources": [],
                })),
            )
            .await;
        assert_eq!(
            (rules.status, created.status),
            (200, 200),
            "case {resource_type:?}: expected both surfaces to accept a catalog type, got rules {} / create {}",
            rules.status,
            created.status
        );
    }

    // A type outside it: both surfaces refuse it, with the same status and error code.
    for (index, (case, resource_type)) in unsupported_but_pattern_valid()
        .into_iter()
        .enumerate()
    {
        let rules = emulator
            .send("GET", &format!("/rules?resourceType={resource_type}"), None)
            .await;
        let created = emulator
            .send(
                "POST",
                "/resourcesets",
                Some(json!({
                    "resourceSetName": format!("disagree_set_{index}"),
                    "resourceSetType": resource_type,
                    "resources": [],
                })),
            )
            .await;
        assert_eq!(
            (rules.status, created.status),
            (400, 400),
            "case {case:?} ({resource_type:?}): expected both surfaces to refuse, got rules {} / create {}",
            rules.status,
            created.status
        );
        assert_eq!(
            rules.error_code(),
            created.error_code(),
            "case {case:?} ({resource_type:?}): expected one error code from both surfaces, got rules {:?} / create {:?}",
            rules.error_code(),
            created.error_code()
        );
    }
}
