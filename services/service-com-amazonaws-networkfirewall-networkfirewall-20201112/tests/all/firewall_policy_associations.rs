// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AssociateFirewallPolicy`.
//!
//! Every test drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: NetworkFirewall_20201112.<OperationName>` through the same
//! router the emulator boots, so what is asserted is the WIRE behaviour and not a
//! handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{
    error_code, error_message, is_uuid_form, string_at, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// Create a firewall on `policy_arn` and return its create response.
async fn create_firewall(
    harness: &Harness,
    name: &str,
    policy_arn: &str,
    policy_change_protection: bool,
) -> Value {
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": format!("subnet-{name}") }],
                "FirewallPolicyChangeProtection": policy_change_protection,
            }),
        )
        .await
}

/// The `FirewallPolicyArn` the firewall `name` currently reports, read back
/// through `DescribeFirewall`.
async fn described_policy_arn(harness: &Harness, name: &str) -> String {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    string_at(&described, &["Firewall", "FirewallPolicyArn"]).to_string()
}

/// The `UpdateToken` the firewall `name` currently reports.
async fn described_token(harness: &Harness, name: &str) -> String {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    string_at(&described, &["UpdateToken"]).to_string()
}

/// The `NumberOfAssociations` the policy `name` currently reports, read back
/// through `DescribeFirewallPolicy`.
async fn described_associations(harness: &Harness, name: &str) -> i64 {
    let described = harness
        .ok("DescribeFirewallPolicy", &json!({ "FirewallPolicyName": name }))
        .await;
    described["FirewallPolicyResponse"]["NumberOfAssociations"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected FirewallPolicyResponse.NumberOfAssociations in {described:?}"))
}

/// The ARN a policy of `name` carries in the region these tests sign for.
fn policy_arn(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/{name}")
}

/// How the request names the firewall it changes.
#[derive(Debug, Clone, Copy)]
enum Identify {
    /// `FirewallName` alone.
    Name,
    /// `FirewallArn` alone.
    Arn,
    /// Both, naming the same firewall.
    Both,
}

/// One row of the association table.
#[derive(Debug)]
struct AssociateCase {
    firewall_name: &'static str,
    /// The policy the firewall is created on.
    initial_policy: &'static str,
    /// The policy the `AssociateFirewallPolicy` call names.
    requested_policy: &'static str,
    identify: Identify,
}

/// RULE: `AssociateFirewallPolicy` makes the firewall's one policy association
/// the firewall policy the request references — re-minting the firewall's
/// optimistic-locking token when that association moved — and the firewall then
/// reports exactly that policy to any later reader.
///
/// The rows vary what the rule generalises over: which policy the firewall starts
/// on, whether the requested policy differs from it, and how the request
/// identifies the firewall (by name, by ARN, or by both).
#[tokio::test]
async fn associate_firewall_policy_repoints_the_firewall_at_the_named_policy() {
    let cases = [
        AssociateCase {
            firewall_name: "assoc-pol-move",
            initial_policy: "pol-a",
            requested_policy: "pol-b",
            identify: Identify::Name,
        },
        AssociateCase {
            firewall_name: "assoc-pol-by-arn",
            initial_policy: "pol-a",
            requested_policy: "pol-c",
            identify: Identify::Arn,
        },
        AssociateCase {
            firewall_name: "assoc-pol-by-both",
            initial_policy: "pol-b",
            requested_policy: "pol-a",
            identify: Identify::Both,
        },
        AssociateCase {
            firewall_name: "assoc-pol-same",
            initial_policy: "pol-c",
            requested_policy: "pol-c",
            identify: Identify::Name,
        },
    ];

    let harness = Harness::new();
    for policy in ["pol-a", "pol-b", "pol-c"] {
        let created = harness.shared_policy(policy).await;
        assert_eq!(
            created,
            policy_arn(policy),
            "case policy {policy}: expected the created policy's ARN to carry the request's \
             region and name, got {created:?}"
        );
    }

    for case in &cases {
        create_firewall(&harness, case.firewall_name, &policy_arn(case.initial_policy), false).await;
        let token_before = described_token(&harness, case.firewall_name).await;
        let firewall_arn = format!(
            "arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{}",
            case.firewall_name
        );

        let mut request = json!({ "FirewallPolicyArn": policy_arn(case.requested_policy) });
        match case.identify {
            Identify::Name => {
                request["FirewallName"] = json!(case.firewall_name);
            }
            Identify::Arn => {
                request["FirewallArn"] = json!(firewall_arn);
            }
            Identify::Both => {
                request["FirewallName"] = json!(case.firewall_name);
                request["FirewallArn"] = json!(firewall_arn);
            }
        }
        let associated = harness.ok("AssociateFirewallPolicy", &request).await;

        assert_eq!(
            string_at(&associated, &["FirewallArn"]),
            firewall_arn,
            "case {case:?}: expected the response to identify the firewall by ARN, got \
             {associated:?}"
        );
        assert_eq!(
            string_at(&associated, &["FirewallName"]),
            case.firewall_name,
            "case {case:?}: expected the response to identify the firewall by name, got \
             {associated:?}"
        );
        assert_eq!(
            string_at(&associated, &["FirewallPolicyArn"]),
            policy_arn(case.requested_policy),
            "case {case:?}: expected the response to report the associated policy, got \
             {associated:?}"
        );

        // The response is not a per-call projection: a later reader sees the same
        // association.
        let read_back = described_policy_arn(&harness, case.firewall_name).await;
        assert_eq!(
            read_back,
            policy_arn(case.requested_policy),
            "case {case:?}: expected DescribeFirewall to report the associated policy, got \
             {read_back:?}"
        );

        // `FirewallStatus.SyncStates[zone].Config` is keyed by the ARN of the
        // configuration object the endpoint syncs, which is this firewall's
        // policy, so it moves with the association too.
        let described = harness
            .ok(
                "DescribeFirewall",
                &json!({ "FirewallName": case.firewall_name }),
            )
            .await;
        for (zone, sync_state) in described["FirewallStatus"]["SyncStates"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected SyncStates in {described:?}"))
        {
            let keys: Vec<&String> = sync_state["Config"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: expected Config in {sync_state:?}"))
                .keys()
                .collect();
            assert_eq!(
                keys,
                vec![&policy_arn(case.requested_policy)],
                "case {case:?} zone {zone}: expected the endpoint's Config keyed by the \
                 associated policy, got {keys:?}"
            );
        }

        // The token marks the state of the resource: it moves when the call
        // changed the association, and stays when the firewall already used the
        // named policy.
        let token_after = string_at(&associated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&token_after),
            "case {case:?}: expected UpdateToken in the modelled UUID form, got {token_after:?}"
        );
        assert_eq!(
            token_after != token_before,
            case.initial_policy != case.requested_policy,
            "case {case:?}: expected the UpdateToken to change exactly when the association did \
             (before {token_before:?}, after {token_after:?})"
        );
        assert_eq!(
            described_token(&harness, case.firewall_name).await,
            token_after,
            "case {case:?}: expected the firewall to keep handing out the token the association \
             reported"
        );
    }
}

/// RULE: a firewall policy's `NumberOfAssociations` counts the firewalls
/// currently associated with it, so each association moves the count off the
/// policy the firewall used and onto the one it now uses — and a policy no
/// firewall is left using can be deleted, while one still in use cannot.
#[tokio::test]
async fn an_association_moves_the_policy_association_count() {
    let harness = Harness::new();
    harness.shared_policy("count-from").await;
    harness.shared_policy("count-to").await;

    let firewalls = ["count-fw-1", "count-fw-2", "count-fw-3"];
    for name in firewalls {
        create_firewall(&harness, name, &policy_arn("count-from"), false).await;
    }
    assert_eq!(
        described_associations(&harness, "count-from").await,
        firewalls.len() as i64,
        "case after create: expected every firewall counted on the policy it was created on"
    );
    assert_eq!(
        described_associations(&harness, "count-to").await,
        0,
        "case after create: expected the unused policy to count no associations"
    );

    for (moved, name) in firewalls.iter().enumerate() {
        harness
            .ok(
                "AssociateFirewallPolicy",
                &json!({
                    "FirewallName": name,
                    "FirewallPolicyArn": policy_arn("count-to"),
                }),
            )
            .await;
        let moved = moved as i64 + 1;
        assert_eq!(
            described_associations(&harness, "count-from").await,
            firewalls.len() as i64 - moved,
            "case moved {moved} of {}: expected the source policy's count to drop",
            firewalls.len()
        );
        assert_eq!(
            described_associations(&harness, "count-to").await,
            moved,
            "case moved {moved} of {}: expected the target policy's count to rise",
            firewalls.len()
        );

        // Re-associating the policy the firewall already uses associates nothing,
        // so neither count moves.
        harness
            .ok(
                "AssociateFirewallPolicy",
                &json!({
                    "FirewallName": name,
                    "FirewallPolicyArn": policy_arn("count-to"),
                }),
            )
            .await;
        assert_eq!(
            described_associations(&harness, "count-to").await,
            moved,
            "case re-associated {name}: expected the count to stay at {moved}"
        );
    }

    // Nothing uses the source policy any more, so it is deletable; the target
    // policy is in use by every firewall, so it is not.
    harness
        .ok(
            "DeleteFirewallPolicy",
            &json!({ "FirewallPolicyName": "count-from" }),
        )
        .await;
    let (status, body) = harness
        .call(
            "DeleteFirewallPolicy",
            &json!({ "FirewallPolicyName": "count-to" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case delete the in-use policy: expected HTTP 400, got {status} with body {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidOperationException",
        "case delete the in-use policy: expected InvalidOperationException, got {body:?}"
    );
}

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    label: &'static str,
    request: Value,
    code: &'static str,
    /// The message, where this service fixes it; `None` where the text is not a
    /// requirement and only the status and code are.
    message: Option<&'static str>,
}

/// RULE: a policy-association request the service cannot carry out is refused
/// with the modelled client error for that defect, and every firewall's policy
/// association is left exactly as it was.
#[tokio::test]
async fn refused_policy_associations_change_nothing() {
    let harness = Harness::new();
    harness.shared_policy("refuse-a").await;
    harness.shared_policy("refuse-b").await;
    create_firewall(&harness, "open-pol-fw", &policy_arn("refuse-a"), false).await;
    create_firewall(&harness, "other-pol-fw", &policy_arn("refuse-a"), false).await;
    create_firewall(&harness, "locked-pol-fw", &policy_arn("refuse-a"), true).await;
    let open_arn = format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/open-pol-fw");
    // A resource that EXISTS but is of the wrong kind: its ARN carries no
    // firewall policy, so it is not a policy this firewall can be associated
    // with either.
    let rule_group_arn = harness
        .shared_rule_group("refuse-rg", "STATEFUL", 10)
        .await;

    let cases = [
        RefusalCase {
            label: "the firewall is protected against policy changes",
            request: json!({
                "FirewallName": "locked-pol-fw",
                "FirewallPolicyArn": policy_arn("refuse-b"),
            }),
            code: "InvalidOperationException",
            message: None,
        },
        RefusalCase {
            label: "a stale update token",
            request: json!({
                "FirewallName": "open-pol-fw",
                "UpdateToken": "00000000-0000-0000-0000-000000000000",
                "FirewallPolicyArn": policy_arn("refuse-b"),
            }),
            code: "InvalidTokenException",
            message: Some("Update token isn't valid."),
        },
        RefusalCase {
            label: "no firewall of that name",
            request: json!({
                "FirewallName": "absent-pol-fw",
                "FirewallPolicyArn": policy_arn("refuse-b"),
            }),
            code: "ResourceNotFoundException",
            message: Some(NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            label: "no firewall identified at all",
            request: json!({ "FirewallPolicyArn": policy_arn("refuse-b") }),
            code: "InvalidRequestException",
            message: Some("At least one of FirewallName or FirewallArn must be set"),
        },
        RefusalCase {
            label: "name and ARN identify different firewalls",
            request: json!({
                "FirewallName": "other-pol-fw",
                "FirewallArn": open_arn,
                "FirewallPolicyArn": policy_arn("refuse-b"),
            }),
            code: "InvalidRequestException",
            message: Some("Name and Arn(Name in Arn) do not match"),
        },
        RefusalCase {
            label: "the policy ARN names no stored policy",
            request: json!({
                "FirewallName": "open-pol-fw",
                "FirewallPolicyArn": "arn:aws:network-firewall:eu-west-1:000000000000:firewall-policy/absent",
            }),
            code: "InvalidRequestException",
            message: Some(
                "firewall policy ARN does not exist, parameter: \
                 [arn:aws:network-firewall:eu-west-1:000000000000:firewall-policy/absent]",
            ),
        },
        RefusalCase {
            label: "the policy ARN names a rule group, not a firewall policy",
            request: json!({
                "FirewallName": "open-pol-fw",
                "FirewallPolicyArn": rule_group_arn,
            }),
            code: "InvalidRequestException",
            message: None,
        },
    ];

    let firewalls = ["open-pol-fw", "other-pol-fw", "locked-pol-fw"];
    for case in &cases {
        let mut before = Vec::new();
        for name in firewalls {
            before.push((described_policy_arn(&harness, name).await, described_token(&harness, name).await));
        }
        let counts_before = (
            described_associations(&harness, "refuse-a").await,
            described_associations(&harness, "refuse-b").await,
        );

        let (status, body) = harness
            .call("AssociateFirewallPolicy", &case.request)
            .await;
        assert_eq!(
            status, 400,
            "case {} ({case:?}): expected HTTP 400, got {status} with body {body:?}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {case:?}: expected error code {}, got {body:?}",
            case.code
        );
        if let Some(message) = case.message {
            assert_eq!(
                error_message(&body),
                message,
                "case {case:?}: expected message {message:?}, got {body:?}"
            );
        }

        let mut after = Vec::new();
        for name in firewalls {
            after.push((described_policy_arn(&harness, name).await, described_token(&harness, name).await));
        }
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave every firewall's policy association and \
             token unchanged, got {after:?}"
        );
        let counts_after = (
            described_associations(&harness, "refuse-a").await,
            described_associations(&harness, "refuse-b").await,
        );
        assert_eq!(
            counts_after, counts_before,
            "case {case:?}: expected the refusal to leave every policy's NumberOfAssociations \
             unchanged, got {counts_after:?}"
        );
    }
}
