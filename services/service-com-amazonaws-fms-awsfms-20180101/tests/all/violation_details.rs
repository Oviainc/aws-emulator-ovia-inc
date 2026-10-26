// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetViolationDetails`.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope — so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the
//! WIRE response.
//!
//! The rules asserted here:
//!
//! 1. The read is organisation-scoped, so with no Firewall Manager administrator
//!    association in the account/Region it is refused with HTTP 400
//!    `AccessDeniedException` — whatever else the request carries, and before any
//!    identifier is looked up.
//! 2. Answered, the `ViolationDetail` is the violation the store records for the
//!    (policy, member account, resource) triple the request names, every member
//!    read off that record: the findings in the order they were recorded, the
//!    resource's recorded tags and its recorded description.
//! 3. The triple must resolve: a `PolicyId` naming no stored policy, a
//!    `MemberAccount` the stored policy does not apply to, a `ResourceId` with no
//!    recorded violation of that policy in that account, and a `ResourceType`
//!    other than the one the violation was recorded under are all the modelled
//!    `ResourceNotFoundException`, with the service's one not-found text.
//! 4. The read is one of three surfaces onto ONE violation inventory: every
//!    resource `GetComplianceDetail` names as a violator has a violation detail,
//!    and nothing is written — repeating a read answers identically and the
//!    inventory is unchanged.
//!
//! The violation inventory the operation reports is seeded through the store:
//! Firewall Manager learns a violation by evaluating the member accounts'
//! resources, this emulator stands in for no such evaluator, and so the only
//! honest source of a violation is one somebody put in the store. Rule 4 seeds it
//! through the seam the compliance suites share
//! ([`crate::compliance_status::seed_violations`]); the rows below seed the
//! per-resource FINDINGS that seam deliberately records none of.
//!
//! Each rule is asserted over a TABLE whose rows differ in resource type, in the
//! form and length of the resource id, in which `ResourceViolation` member
//! carries the finding, in how many findings one resource has (including none),
//! in tag count (including none) and in whether a description was recorded.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    AwsEc2InstanceViolation, AwsEc2NetworkInterfaceViolation, AwsVPCSecurityGroupViolation,
    NetworkFirewallMissingSubnetViolation, PartialMatch, ResourceViolation, Tag,
    WebACLHasOutOfScopeResourcesViolation,
};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the store seam and the compliance fixtures live in one place
// for this test target, so no two suites can drift in how a request is signed,
// the store is seeded or the organisation is set up.
use crate::administrator_gated_operations::with_store;
use crate::compliance_status::{associate, error_message, put_policy, seed_violations, ADMIN};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A `PolicyId` (the model's `com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "99999999-8888-7777-6666-555555555555";

/// A second account of the organisation, so a row's member account is not always
/// the administrator's own.
const MEMBER: &str = "222222222222";

/// An organisation account the policies below do not apply to.
const OUT_OF_SCOPE_ACCOUNT: &str = "333333333333";

/// The instant the seeded violations are recorded at: 2026-01-01T01:00:00Z in
/// Unix seconds.
const VIOLATION_DETECTED_SECS: u64 = 1_767_229_200;

/// `secs` seconds after the Unix epoch.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// The resource types `GetViolationDetails` documents for its `ResourceType`:
/// "Supported resource types are: AWS::WAFv2::WebACL, AWS::EC2::Instance,
/// AWS::EC2::NetworkInterface, AWS::EC2::SecurityGroup,
/// AWS::NetworkFirewall::FirewallPolicy, and AWS::EC2::Subnet"
/// (`smithy_query(query="operation", operation="GetViolationDetails")`, member
/// `ResourceType`). The rows below are drawn from this catalog, which is also
/// where the "another supported type" of the mismatch case comes from.
const SUPPORTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::WAFv2::WebACL",
    "AWS::EC2::Instance",
    "AWS::EC2::NetworkInterface",
    "AWS::EC2::SecurityGroup",
    "AWS::NetworkFirewall::FirewallPolicy",
    "AWS::EC2::Subnet",
];

/// One row of the violation table: a resource of some type, the findings an
/// evaluation recorded against it, and the JSON the wire must then carry for
/// them.
///
/// The recorded findings (`violations`) and the expected wire form
/// (`expected_violations`) are written independently on purpose: the test then
/// pins the projection rather than restating it.
struct Row {
    name: &'static str,
    account: &'static str,
    resource_id: &'static str,
    resource_type: &'static str,
    violations: Vec<ResourceViolation>,
    tags: Option<Vec<Tag>>,
    description: Option<&'static str>,
    expected_violations: Value,
}

/// The class rule 2 is asserted over: one row per `ResourceViolation` member
/// under test, spanning the documented resource types, resource-id forms and
/// lengths (including the model's minimum of one character), finding counts
/// (including none at all), tag counts and a recorded/absent description.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "security-group-rule",
            account: ADMIN,
            resource_id: "sg-0123456789abcdef0",
            resource_type: "AWS::EC2::SecurityGroup",
            violations: vec![ResourceViolation {
                aws_vpc_security_group_violation: Some(AwsVPCSecurityGroupViolation {
                    violation_target: Some("sg-0123456789abcdef0".to_string()),
                    violation_target_description: Some("inbound 0.0.0.0/0 on 22".to_string()),
                    partial_matches: Some(vec![PartialMatch {
                        reference: Some("sg-0999999999999999a".to_string()),
                        target_violation_reasons: Some(vec![
                            "RESOURCE_VIOLATES_AUDIT_SECURITY_GROUP".to_string(),
                        ]),
                    }]),
                    possible_security_group_remediation_actions: None,
                }),
                ..ResourceViolation::default()
            }],
            tags: Some(vec![
                Tag {
                    key: "env".to_string(),
                    value: "prod".to_string(),
                },
                Tag {
                    key: "owner".to_string(),
                    value: "team-a".to_string(),
                },
            ]),
            description: Some("security group of the payments VPC"),
            expected_violations: json!([{
                "AwsVPCSecurityGroupViolation": {
                    "ViolationTarget": "sg-0123456789abcdef0",
                    "ViolationTargetDescription": "inbound 0.0.0.0/0 on 22",
                    "PartialMatches": [{
                        "Reference": "sg-0999999999999999a",
                        "TargetViolationReasons": ["RESOURCE_VIOLATES_AUDIT_SECURITY_GROUP"]
                    }]
                }
            }]),
        },
        Row {
            name: "network-interface",
            account: MEMBER,
            resource_id: "eni-00112233445566778",
            resource_type: "AWS::EC2::NetworkInterface",
            violations: vec![ResourceViolation {
                aws_ec2_network_interface_violation: Some(AwsEc2NetworkInterfaceViolation {
                    violation_target: Some("eni-00112233445566778".to_string()),
                    violating_security_groups: Some(vec![
                        "sg-0123456789abcdef0".to_string(),
                        "sg-00000000000000001".to_string(),
                    ]),
                }),
                ..ResourceViolation::default()
            }],
            tags: Some(Vec::new()),
            description: None,
            expected_violations: json!([{
                "AwsEc2NetworkInterfaceViolation": {
                    "ViolationTarget": "eni-00112233445566778",
                    "ViolatingSecurityGroups": ["sg-0123456789abcdef0", "sg-00000000000000001"]
                }
            }]),
        },
        Row {
            // TWO findings against one resource, so the list's order and
            // cardinality are both asserted.
            name: "instance-with-two-findings",
            account: ADMIN,
            resource_id: "i-0abcdef1234567890",
            resource_type: "AWS::EC2::Instance",
            violations: vec![
                ResourceViolation {
                    aws_ec2_instance_violation: Some(AwsEc2InstanceViolation {
                        violation_target: Some("i-0abcdef1234567890".to_string()),
                        aws_ec2_network_interface_violations: Some(vec![
                            AwsEc2NetworkInterfaceViolation {
                                violation_target: Some("eni-00112233445566778".to_string()),
                                violating_security_groups: Some(vec![
                                    "sg-0123456789abcdef0".to_string()
                                ]),
                            },
                        ]),
                    }),
                    ..ResourceViolation::default()
                },
                ResourceViolation {
                    aws_ec2_network_interface_violation: Some(AwsEc2NetworkInterfaceViolation {
                        violation_target: Some("eni-00998877665544332".to_string()),
                        violating_security_groups: None,
                    }),
                    ..ResourceViolation::default()
                },
            ],
            tags: None,
            description: Some("bastion host"),
            expected_violations: json!([
                {
                    "AwsEc2InstanceViolation": {
                        "ViolationTarget": "i-0abcdef1234567890",
                        "AwsEc2NetworkInterfaceViolations": [{
                            "ViolationTarget": "eni-00112233445566778",
                            "ViolatingSecurityGroups": ["sg-0123456789abcdef0"]
                        }]
                    }
                },
                {
                    "AwsEc2NetworkInterfaceViolation": {
                        "ViolationTarget": "eni-00998877665544332"
                    }
                }
            ]),
        },
        Row {
            name: "subnet-missing-firewall-subnet",
            account: MEMBER,
            resource_id: "subnet-0a1b2c3d",
            resource_type: "AWS::EC2::Subnet",
            violations: vec![ResourceViolation {
                network_firewall_missing_subnet_violation: Some(
                    NetworkFirewallMissingSubnetViolation {
                        violation_target: Some("subnet-0a1b2c3d".to_string()),
                        vpc: Some("vpc-0f0f0f0f".to_string()),
                        availability_zone: Some("us-west-2b".to_string()),
                        // From the model's own catalog for
                        // `ViolationReason`/`TargetViolationReason`
                        // (`ops::types::VIOLATION_REASON_VALUES`).
                        target_violation_reason: Some("MISSING_FIREWALL_SUBNET_IN_AZ".to_string()),
                    },
                ),
                ..ResourceViolation::default()
            }],
            tags: Some(vec![Tag {
                key: "tier".to_string(),
                value: "private".to_string(),
            }]),
            description: None,
            expected_violations: json!([{
                "NetworkFirewallMissingSubnetViolation": {
                    "ViolationTarget": "subnet-0a1b2c3d",
                    "VPC": "vpc-0f0f0f0f",
                    "AvailabilityZone": "us-west-2b",
                    "TargetViolationReason": "MISSING_FIREWALL_SUBNET_IN_AZ"
                }
            }]),
        },
        Row {
            // A resource id in ARN form: the model's `ResourceId` pattern
            // (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) admits the `:` and `/` an ARN
            // carries.
            name: "web-acl-out-of-scope-resources",
            account: ADMIN,
            resource_id: "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/demo/1111",
            resource_type: "AWS::WAFv2::WebACL",
            violations: vec![ResourceViolation {
                web_acl_has_out_of_scope_resources_violation: Some(
                    WebACLHasOutOfScopeResourcesViolation {
                        web_acl_arn: Some(
                            "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/demo/1111"
                                .to_string(),
                        ),
                        out_of_scope_resource_list: Some(vec![
                            "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/x/1"
                                .to_string(),
                        ]),
                    },
                ),
                ..ResourceViolation::default()
            }],
            tags: None,
            description: Some("regional web ACL"),
            expected_violations: json!([{
                "WebACLHasOutOfScopeResourcesViolation": {
                    "WebACLArn": "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/demo/1111",
                    "OutOfScopeResourceList": [
                        "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/x/1"
                    ]
                }
            }]),
        },
        Row {
            // A violation recorded with NO finding of this shape, and a resource
            // id of the model's minimum length (`ResourceId` is 1..1024): the
            // detail reports an empty list rather than a finding worded by the
            // emulator.
            name: "no-finding-recorded",
            account: MEMBER,
            resource_id: "x",
            resource_type: "AWS::NetworkFirewall::FirewallPolicy",
            violations: Vec::new(),
            tags: None,
            description: None,
            expected_violations: json!([]),
        },
    ]
}

/// Record one row's violation in the store, the way an evaluation records one:
/// the same [`state::PolicyViolationRecord`] and the same key function
/// ([`state::PolicyViolationRecord::key`]) the compliance seam
/// ([`crate::compliance_status::seed_violations`]) writes, carrying in addition
/// the per-resource findings that seam records none of.
fn record_violation(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    policy_id: &str,
    row: &Row,
) {
    with_store(engine, |store| {
        let record = state::PolicyViolationRecord {
            policy_id: policy_id.to_string(),
            account_id: row.account.to_string(),
            resource_id: row.resource_id.to_string(),
            resource_type: Some(row.resource_type.to_string()),
            // From the model's own catalog for `ViolationReason`
            // (`ops::types::VIOLATION_REASON_VALUES`).
            violation_reason: Some("RESOURCE_VIOLATES_AUDIT_SECURITY_GROUP".to_string()),
            metadata: None,
            detected_at: at(VIOLATION_DETECTED_SECS),
            resource_violations: row.violations.clone(),
            resource_tags: row.tags.clone(),
            resource_description: row.description.map(str::to_string),
        };
        store.policy_violations.insert(record.key(), record);
    });
}

/// Call `GetViolationDetails` for one (policy, account, resource, type) request.
async fn details(
    router: &MultiServiceRouter,
    policy_id: &str,
    account: &str,
    resource_id: &str,
    resource_type: &str,
) -> (u16, Value) {
    call(
        router,
        "GetViolationDetails",
        &json!({
            "PolicyId": policy_id,
            "MemberAccount": account,
            "ResourceId": resource_id,
            "ResourceType": resource_type,
        }),
    )
    .await
}

/// The `ViolationDetail` of a response, asserting the member is there at all.
fn reported(case: &str, body: &Value) -> Value {
    let reported = &body["ViolationDetail"];
    assert!(
        reported.is_object(),
        "case {case}: no ViolationDetail in {body}"
    );
    reported.clone()
}

/// `value` with every null object member dropped, recursively.
///
/// An optional member a response leaves out and one it sends as null are the same
/// thing to a client, so the expectations below name only the members a row
/// actually recorded and the comparison is made on this normalised form.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// The member `name` of a reported detail, as `Value::Null` when it is absent:
/// an absent optional member is what a row that recorded nothing for it expects.
fn member(reported: &Value, name: &str) -> Value {
    without_nulls(&reported.get(name).cloned().unwrap_or(Value::Null))
}

/// The expected wire form of a row's recorded resource tags.
fn expected_tags(row: &Row) -> Value {
    match row.tags.as_ref() {
        None => Value::Null,
        Some(tags) => Value::Array(
            tags.iter()
                .map(|tag| json!({ "Key": tag.key, "Value": tag.value }))
                .collect(),
        ),
    }
}

/// Store one policy scoped to the accounts the rows record violations in, and
/// return the `PolicyId` the service minted.
async fn put_scoped_policy(router: &MultiServiceRouter, name: &str) -> String {
    put_policy(
        router,
        name,
        Some(json!({ "ACCOUNT": [ADMIN, MEMBER] })),
        None,
    )
    .await
}

/// Rule 1: no administrator association, no organisation-scoped answer — and the
/// refusal does not depend on what else the request carries, nor on whether the
/// policy, the account or the resource exists.
#[tokio::test]
async fn get_violation_details_is_refused_until_an_administrator_is_associated() {
    let (router, engine) = emulator_with_engine();
    // A stored policy with a recorded violation, so the gate is shown to run
    // before any identifier is looked up: the request that WOULD be answered and
    // the ones that would not are refused identically. Writing a policy is
    // organisation-scoped too, so it is created with an administrator associated
    // and the association is then removed through the store seam — which is the
    // precondition this rule is about.
    associate(&router, ADMIN).await;
    let policy_id = put_scoped_policy(&router, "gated").await;
    let table = rows();
    let answerable = &table[0];
    record_violation(&engine, &policy_id, answerable);
    with_store(&engine, |store| store.admin_account = None);

    let shapes = [
        (
            "recorded-violation",
            policy_id.clone(),
            answerable.account.to_string(),
            answerable.resource_id.to_string(),
            answerable.resource_type.to_string(),
        ),
        (
            "unknown-resource",
            policy_id.clone(),
            answerable.account.to_string(),
            "sg-0000000000000dead".to_string(),
            answerable.resource_type.to_string(),
        ),
        (
            "out-of-scope-account",
            policy_id.clone(),
            OUT_OF_SCOPE_ACCOUNT.to_string(),
            answerable.resource_id.to_string(),
            answerable.resource_type.to_string(),
        ),
        (
            "unknown-policy",
            UNKNOWN_POLICY_ID.to_string(),
            answerable.account.to_string(),
            answerable.resource_id.to_string(),
            answerable.resource_type.to_string(),
        ),
        (
            "shortest-account-id",
            policy_id.clone(),
            "7".to_string(),
            answerable.resource_id.to_string(),
            answerable.resource_type.to_string(),
        ),
    ];

    for (shape, policy, account, resource_id, resource_type) in &shapes {
        let (status, body) = details(&router, policy, account, resource_id, resource_type).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {shape}: with no administrator association the call must be refused, \
             got {status} with {body}"
        );
    }

    // The refusal's message is built from the request: it names the calling
    // account and the Region the request was signed for.
    for region in [REGION, "eu-central-1", "ap-southeast-4"] {
        let (status, body) = call_in(
            &router,
            "GetViolationDetails",
            &json!({
                "PolicyId": policy_id,
                "MemberAccount": answerable.account,
                "ResourceId": answerable.resource_id,
                "ResourceType": answerable.resource_type,
            }),
            region,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {region}: expected the authorization refusal, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("No default admin could be found for account {ACCOUNT} in Region {region}"),
            "case {region}: the refusal must name the calling account and the Region of the \
             request, got {body}"
        );
    }

    // With the administrator associated again, the same shapes are judged on
    // their own merits: only the recorded violation is answered.
    associate(&router, ADMIN).await;
    for (shape, policy, account, resource_id, resource_type) in &shapes {
        let (status, body) = details(&router, policy, account, resource_id, resource_type).await;
        if *shape == "recorded-violation" {
            assert_eq!(
                status, 200,
                "case {shape}: an associated administrator must be answered, got {status} \
                 with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {shape}: expected the modelled not-found, got {status} with {body}"
            );
        }
    }
}

/// Rules 2 and 3: the detail reported for a recorded violation is that record,
/// and it is reported for exactly the resource and resource type it was recorded
/// under.
#[tokio::test]
async fn a_recorded_violation_is_reported_for_the_resource_and_type_it_was_recorded_under() {
    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let policy_id = put_scoped_policy(&router, "violating").await;

    // Every row of the catalog is drawn from the operation's own documented set
    // of resource types.
    for row in rows() {
        assert!(
            SUPPORTED_RESOURCE_TYPES.contains(&row.resource_type),
            "case {}: the row's resource type must come from the documented catalog, got {}",
            row.name,
            row.resource_type
        );
        record_violation(&engine, &policy_id, &row);
    }

    for row in rows() {
        let case = row.name;
        let (status, body) = details(
            &router,
            &policy_id,
            row.account,
            row.resource_id,
            row.resource_type,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a recorded violation must be reported, got {status} with {body}"
        );
        let reported = reported(case, &body);
        assert_eq!(
            (
                reported["PolicyId"].as_str(),
                reported["MemberAccount"].as_str(),
                reported["ResourceId"].as_str(),
                reported["ResourceType"].as_str(),
            ),
            (
                Some(policy_id.as_str()),
                Some(row.account),
                Some(row.resource_id),
                Some(row.resource_type),
            ),
            "case {case}: the detail must identify the resource it was recorded for, \
             got {reported}"
        );
        assert_eq!(
            member(&reported, "ResourceViolations"),
            row.expected_violations,
            "case {case}: expected the recorded findings, got {reported}"
        );
        assert_eq!(
            member(&reported, "ResourceTags"),
            expected_tags(&row),
            "case {case}: expected the recorded resource tags, got {reported}"
        );
        assert_eq!(
            member(&reported, "ResourceDescription"),
            row.description.map_or(Value::Null, Value::from),
            "case {case}: expected the recorded resource description, got {reported}"
        );

        // Rule 3: the same resource under another type of the documented catalog
        // is a resource no violation was recorded for.
        for other_type in SUPPORTED_RESOURCE_TYPES
            .iter()
            .filter(|candidate| **candidate != row.resource_type)
        {
            let (status, body) = details(
                &router,
                &policy_id,
                row.account,
                row.resource_id,
                other_type,
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}/{other_type}: a violation recorded under another resource type \
                 must not be reported, got {status} with {body}"
            );
        }

        // Rule 3: so is this row's type on a resource id nothing was recorded
        // for, and the row's resource in an account it was not recorded in.
        let unrecorded = format!("{}-unrecorded", row.resource_id);
        let (status, body) = details(
            &router,
            &policy_id,
            row.account,
            &unrecorded,
            row.resource_type,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: an unrecorded resource id must be not found, got {status} with {body}"
        );
        let other_account = if row.account == ADMIN { MEMBER } else { ADMIN };
        let (status, body) = details(
            &router,
            &policy_id,
            other_account,
            row.resource_id,
            row.resource_type,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: the resource must not be reported for an account it was not \
             recorded in, got {status} with {body}"
        );
    }
}

/// Rule 3: the policy and the member account must resolve too — an unknown or
/// deleted policy, and an account the policy does not apply to, have no violation
/// detail, and the refusal carries the service's one not-found text.
#[tokio::test]
async fn an_unresolved_policy_or_account_has_no_violation_detail() {
    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let policy_id = put_scoped_policy(&router, "to-be-deleted").await;
    let table = rows();
    let row = &table[0];
    record_violation(&engine, &policy_id, row);

    // Well-formed `PolicyId`s (36 characters of `^[a-z0-9A-Z-]{36}$`) that name
    // nothing: a UUID shape, an all-digit one and an all-dash one.
    for unknown in [
        UNKNOWN_POLICY_ID,
        "000000000000000000000000000000000000",
        "------------------------------------",
    ] {
        let (status, body) = details(
            &router,
            unknown,
            row.account,
            row.resource_id,
            row.resource_type,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {unknown}: expected the modelled not-found, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {unknown}: expected the service's not-found text, got {body}"
        );
    }

    // Accounts outside the policy's own scope: the policy names ADMIN and MEMBER,
    // so no other organisation account was evaluated against it.
    for account in [OUT_OF_SCOPE_ACCOUNT, "444444444444", "7"] {
        let (status, body) =
            details(&router, &policy_id, account, row.resource_id, row.resource_type).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {account}: an account the policy does not apply to must be not found, \
             got {status} with {body}"
        );
    }

    // The stored policy answers until it is deleted, and not afterwards.
    let (status, body) = details(
        &router,
        &policy_id,
        row.account,
        row.resource_id,
        row.resource_type,
    )
    .await;
    assert_eq!(
        status, 200,
        "case stored: the recorded violation must be reported, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "DeletePolicy",
        &json!({ "PolicyId": policy_id, "DeleteAllPolicyResources": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePolicy must answer 200, got {status} with {body}"
    );
    let (status, body) = details(
        &router,
        &policy_id,
        row.account,
        row.resource_id,
        row.resource_type,
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case deleted: a deleted policy must be not found, got {status} with {body}"
    );
}

/// Rule 4: `GetComplianceDetail` and `GetViolationDetails` are two surfaces onto
/// ONE violation inventory — every resource the compliance detail names as a
/// violator has a violation detail — and neither read writes anything.
#[tokio::test]
async fn every_named_violator_has_a_violation_detail_and_the_read_writes_nothing() {
    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let policy_id = put_scoped_policy(&router, "two-surfaces").await;

    // Seeded through the seam the compliance suites share, which records a
    // violation with no per-resource finding: those resources are named by the
    // compliance detail and answered here with an empty finding list.
    let seeded: usize = 3;
    seed_violations(
        &engine,
        &policy_id,
        ADMIN,
        seeded,
        at(VIOLATION_DETECTED_SECS),
    );
    // And one row of this suite's own table, so the pairing holds for a violation
    // that DOES carry findings.
    let table = rows();
    let row = table
        .iter()
        .find(|row| row.account == ADMIN)
        .expect("a row recorded in the administrator's own account");
    record_violation(&engine, &policy_id, row);

    let (status, body) = call(
        &router,
        "GetComplianceDetail",
        &json!({ "PolicyId": policy_id, "MemberAccount": ADMIN }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetComplianceDetail must answer 200, got {status} with {body}"
    );
    let violators = body["PolicyComplianceDetail"]["Violators"]
        .as_array()
        .unwrap_or_else(|| panic!("the compliance detail names no violators: {body}"))
        .clone();
    assert_eq!(
        violators.len(),
        seeded + 1,
        "the compliance detail must name every recorded violation, got {violators:?}"
    );

    for violator in &violators {
        let resource_id = violator["ResourceId"]
            .as_str()
            .unwrap_or_else(|| panic!("a violator carries no ResourceId: {violator}"));
        let resource_type = violator["ResourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("a violator carries no ResourceType: {violator}"));
        let (status, body) = details(&router, &policy_id, ADMIN, resource_id, resource_type).await;
        assert_eq!(
            status, 200,
            "case {resource_id}: a named violator must have a violation detail, got {status} \
             with {body}"
        );
        let first = reported(resource_id, &body);
        // Rule 4: the read is reporting, not mutating — the second read of the
        // same resource answers exactly what the first did.
        let (status, body) = details(&router, &policy_id, ADMIN, resource_id, resource_type).await;
        assert_eq!(
            (status, reported(resource_id, &body)),
            (200, first.clone()),
            "case {resource_id}: repeating the read must answer identically, got {body}"
        );
        if resource_id == row.resource_id {
            assert_eq!(
                member(&first, "ResourceViolations"),
                row.expected_violations,
                "case {resource_id}: expected the recorded findings, got {first}"
            );
        } else {
            assert_eq!(
                member(&first, "ResourceViolations"),
                json!([]),
                "case {resource_id}: a violation recorded with no finding must report none, \
                 got {first}"
            );
        }
    }

    // Nothing was written: the inventory still holds exactly what was seeded.
    let inventory = with_store(&engine, |store| store.policy_violations.len());
    assert_eq!(
        inventory,
        seeded + 1,
        "reading violation details must not change the inventory, got {inventory} entries"
    );
}
