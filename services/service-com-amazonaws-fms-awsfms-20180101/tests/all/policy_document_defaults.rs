// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the documented DEFAULTS of a stored `Policy` document.
//!
//! A modelled member whose documentation states an explicit default value is not
//! "unset" when the request omits it: the resource behaves by that default, so
//! the service stores it and every reader of the resource is answered with it.
//! `Policy$ResourceTagLogicalOperator` is the member of `Policy` that states one
//! ("Default: `AND`"); the rule is asserted here over a table of policy
//! documents that differ in security-service type, in resource-type form, in
//! whether and how many `ResourceTags` they carry, and in whether they name the
//! member at all — and on both paths a client reads a policy through, the
//! `PutPolicy` response and `GetPolicy`.
//!
//! The wire harness ([`call`], [`emulator_with_administrator`]) is the one the
//! `policy_lifecycle` suite owns, so these tests drive the same
//! deserialize → handle → serialize walk a client does.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::RESOURCE_TAG_LOGICAL_OPERATOR_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_lifecycle::{call, emulator_with_administrator};

/// The value `Policy$ResourceTagLogicalOperator` documents as its default:
/// "Default: `AND`".
const DOCUMENTED_DEFAULT: &str = "AND";

/// One row: a name for the assertion messages, the `Policy` document a client
/// sends, and the `ResourceTagLogicalOperator` the policy therefore has — the
/// value sent, or the documented default when the document names none.
struct Case {
    name: &'static str,
    policy: Value,
    expected: &'static str,
}

/// The base policy documents the member is varied over. They differ in
/// security-service type, in resource-type form (a single `ResourceType` vs a
/// `ResourceTypeList`), in whether they carry `ResourceTags` and how many, and
/// in which other optional members are present.
fn base_documents() -> Vec<(&'static str, Value)> {
    vec![
        (
            // The minimum a policy can be: no `ResourceTags` at all, so the
            // operator applies to an empty tag set.
            "minimal-wafv2",
            json!({
                "PolicyName": "defaults-wafv2",
                "SecurityServicePolicyData": { "Type": "WAFV2" },
                "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
                "ExcludeResourceTags": false,
                "RemediationEnabled": false
            }),
        ),
        (
            // Two resource tags to combine, a `ResourceTypeList`, and the
            // booleans the other way round.
            "shield-two-resource-tags",
            json!({
                "PolicyName": "defaults-shield",
                "SecurityServicePolicyData": { "Type": "SHIELD_ADVANCED" },
                "ResourceType": "ResourceTypeList",
                "ResourceTypeList": [
                    "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    "AWS::EC2::EIP"
                ],
                "ResourceTags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "tier", "Value": "edge" }
                ],
                "ExcludeResourceTags": true,
                "RemediationEnabled": true,
                "DeleteUnusedFMManagedResources": true,
                "PolicyDescription": "shield advanced over two resource types"
            }),
        ),
        (
            // One resource tag, organisation scoping by IncludeMap.
            "security-groups-one-resource-tag",
            json!({
                "PolicyName": "defaults-sg",
                "SecurityServicePolicyData": { "Type": "SECURITY_GROUPS_COMMON" },
                "ResourceType": "AWS::EC2::SecurityGroup",
                "ResourceTags": [{ "Key": "owner", "Value": "team-a" }],
                "IncludeMap": { "ACCOUNT": ["111111111111"] },
                "ExcludeResourceTags": false,
                "RemediationEnabled": true
            }),
        ),
        (
            // A nested PolicyOption and an empty `ResourceTags` list.
            "network-firewall-empty-resource-tags",
            json!({
                "PolicyName": "defaults-nfw",
                "SecurityServicePolicyData": {
                    "Type": "NETWORK_FIREWALL",
                    "PolicyOption": {
                        "NetworkFirewallPolicy": { "FirewallDeploymentModel": "CENTRALIZED" }
                    }
                },
                "ResourceType": "AWS::EC2::VPC",
                "ResourceTags": [],
                "ExcludeResourceTags": false,
                "RemediationEnabled": false
            }),
        ),
    ]
}

/// Every base document in all three member states: absent (the default
/// applies), and each value of the model's own catalog
/// `ops::types::RESOURCE_TAG_LOGICAL_OPERATOR_VALUES`
/// (`com.amazonaws.fms#ResourceTagLogicalOperator`) sent explicitly — including
/// `AND`, which the issue's reproduction did not send.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (name, document) in base_documents() {
        cases.push(Case {
            name,
            policy: document.clone(),
            expected: DOCUMENTED_DEFAULT,
        });
        for sent in RESOURCE_TAG_LOGICAL_OPERATOR_VALUES {
            let mut policy = document.clone();
            policy["ResourceTagLogicalOperator"] = json!(sent);
            policy["PolicyName"] = json!(format!(
                "{}-{}",
                document["PolicyName"].as_str().unwrap_or_default(),
                sent.to_lowercase()
            ));
            cases.push(Case {
                name,
                policy,
                expected: sent,
            });
        }
    }
    cases
}

/// Create one case's policy and return the `PutPolicy` response body.
async fn put(router: &MultiServiceRouter, policy: &Value, case: &str) -> Value {
    let (status, body) = call(router, "PutPolicy", &json!({ "Policy": policy })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutPolicy to answer 200, got {status} with {body}"
    );
    body
}

/// The documented default is a value the model's catalog declares — the rule
/// materialises a MODELLED value, not a string of its own.
#[tokio::test]
async fn the_documented_default_is_a_value_the_model_declares() {
    assert!(
        RESOURCE_TAG_LOGICAL_OPERATOR_VALUES.contains(&DOCUMENTED_DEFAULT),
        "the documented default {DOCUMENTED_DEFAULT:?} must be in the model's \
         ResourceTagLogicalOperator catalog {RESOURCE_TAG_LOGICAL_OPERATOR_VALUES:?}"
    );
}

/// Every row's effective `ResourceTagLogicalOperator` is reported by the
/// `PutPolicy` response and by `GetPolicy`: the value the document sent, or the
/// documented default when it sent none.
#[tokio::test]
async fn a_policy_reports_the_documented_default_when_the_request_names_no_operator() {
    let router = emulator_with_administrator().await;

    for case in cases() {
        let sent = case.policy.get("ResourceTagLogicalOperator").cloned();
        let created = put(&router, &case.policy, case.name).await;
        let policy_id = created["Policy"]["PolicyId"]
            .as_str()
            .expect("a minted PolicyId")
            .to_string();

        assert_eq!(
            created["Policy"]["ResourceTagLogicalOperator"],
            json!(case.expected),
            "case {:?} (sent {sent:?}): PutPolicy must report {:?} as the policy's \
             ResourceTagLogicalOperator, got {:?}",
            case.name,
            case.expected,
            created["Policy"]["ResourceTagLogicalOperator"]
        );

        let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetPolicy to answer 200, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got["Policy"]["ResourceTagLogicalOperator"],
            json!(case.expected),
            "case {:?} (sent {sent:?}): GetPolicy must report {:?} as the policy's \
             ResourceTagLogicalOperator, got {:?}",
            case.name,
            case.expected,
            got["Policy"]["ResourceTagLogicalOperator"]
        );
        // The projection every reader goes through cannot disagree with itself.
        assert_eq!(
            got["Policy"], created["Policy"],
            "case {:?}: GetPolicy must report the policy PutPolicy stored, got {}",
            case.name, got["Policy"]
        );
    }
}

/// `PutPolicy` writes the WHOLE document, so the default follows the request
/// that is current: an update that drops the member has the default again, and
/// one that names a value has that value — in either direction.
#[tokio::test]
async fn an_update_that_drops_the_operator_has_the_documented_default_again() {
    let router = emulator_with_administrator().await;

    for (name, document) in base_documents() {
        for (first, second) in [
            (Some("OR"), None),
            (Some("AND"), Some("OR")),
            (None, Some("OR")),
            (None, None),
        ] {
            let mut create = document.clone();
            create["PolicyName"] = json!(format!(
                "{}-update-{}-{}",
                document["PolicyName"].as_str().unwrap_or_default(),
                first.unwrap_or("absent").to_lowercase(),
                second.unwrap_or("absent").to_lowercase()
            ));
            if let Some(first) = first {
                create["ResourceTagLogicalOperator"] = json!(first);
            }
            let case = format!("{name}: {first:?} then {second:?}");
            let created = put(&router, &create, &case).await;
            let policy_id = created["Policy"]["PolicyId"]
                .as_str()
                .expect("a minted PolicyId")
                .to_string();
            let token = created["Policy"]["PolicyUpdateToken"]
                .as_str()
                .expect("a minted PolicyUpdateToken")
                .to_string();
            assert_eq!(
                created["Policy"]["ResourceTagLogicalOperator"],
                json!(first.unwrap_or(DOCUMENTED_DEFAULT)),
                "case {case:?}: the created policy must carry {:?}, got {:?}",
                first.unwrap_or(DOCUMENTED_DEFAULT),
                created["Policy"]["ResourceTagLogicalOperator"]
            );

            let mut update = create.clone();
            update["PolicyId"] = json!(policy_id);
            update["PolicyUpdateToken"] = json!(token);
            match second {
                Some(value) => update["ResourceTagLogicalOperator"] = json!(value),
                None => {
                    update
                        .as_object_mut()
                        .expect("the policy document is an object")
                        .remove("ResourceTagLogicalOperator");
                }
            }
            let updated = put(&router, &update, &case).await;
            let expected = second.unwrap_or(DOCUMENTED_DEFAULT);
            assert_eq!(
                updated["Policy"]["ResourceTagLogicalOperator"],
                json!(expected),
                "case {case:?}: the updated policy must carry {expected:?}, got {:?}",
                updated["Policy"]["ResourceTagLogicalOperator"]
            );

            let (_, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
            assert_eq!(
                got["Policy"]["ResourceTagLogicalOperator"],
                json!(expected),
                "case {case:?}: GetPolicy must report {expected:?} after the update, got {:?}",
                got["Policy"]["ResourceTagLogicalOperator"]
            );
        }
    }
}
