// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a firewall's PROTECTION SETTINGS: the refusal a
//! change-protection setting answers, the operations that toggle a setting on a
//! live firewall, and what a call addressed to a firewall that is gone gets.
//!
//! Three rules:
//!
//! * a change one of a firewall's change-protection settings blocks is refused
//!   with `InvalidOperationException` (HTTP 400) whose message names the call
//!   that was refused and the setting that refused it —
//!   `Cannot <Operation> because <Member> is on` — and carries nothing else from
//!   the request;
//! * a protection setting is a stored property of the firewall: the operation
//!   that modifies it persists the value, echoes it back with the token the
//!   resource then carries, and the guarded call honours the STORED value;
//! * a name or ARN that names no stored firewall locates no resource, so every
//!   call that names one — a read or a mutation — is refused with
//!   `ResourceNotFoundException` (HTTP 400) and this service's one fixed
//!   not-found text.
//!
//! The message texts below are written out as literals rather than read from
//! `state`, so these tests pin the WIRE text instead of restating the code.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`.

use serde_json::{json, Value};

use crate::harness::{
    error_code, error_message, is_uuid_form, string_at, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// The firewall ARN this emulator builds for `name`, as every ARN assertion in
/// this target builds it.
fn firewall_arn(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{name}")
}

/// Create a firewall on `policy_arn` carrying `subnets` and the three protection
/// settings, and return its create response.
async fn create_firewall(
    harness: &Harness,
    name: &str,
    policy_arn: &str,
    subnets: &[&str],
    protections: Protections,
) -> Value {
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": subnets
                    .iter()
                    .map(|id| json!({ "SubnetId": id }))
                    .collect::<Vec<_>>(),
                "DeleteProtection": protections.delete,
                "SubnetChangeProtection": protections.subnet_change,
                "FirewallPolicyChangeProtection": protections.policy_change,
            }),
        )
        .await
}

/// The three protection settings `CreateFirewall` takes, as one row value.
#[derive(Debug, Clone, Copy, Default)]
struct Protections {
    delete: bool,
    subnet_change: bool,
    policy_change: bool,
}

/// The `Firewall` member `member` of the firewall `name`, read back through
/// `DescribeFirewall`.
async fn described_flag(harness: &Harness, name: &str, member: &str) -> bool {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    described["Firewall"][member]
        .as_bool()
        .unwrap_or_else(|| panic!("expected Firewall.{member} in {described:?}"))
}

/// The firewall `name`'s current `UpdateToken` and the subnet ids and policy ARN
/// it reports, which is everything a refusal must leave untouched.
async fn described_state(harness: &Harness, name: &str) -> (String, Vec<String>, String) {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    let subnets = described["Firewall"]["SubnetMappings"]
        .as_array()
        .unwrap_or_else(|| panic!("expected Firewall.SubnetMappings in {described:?}"))
        .iter()
        .map(|mapping| string_at(mapping, &["SubnetId"]).to_string())
        .collect();
    (
        string_at(&described, &["UpdateToken"]).to_string(),
        subnets,
        string_at(&described, &["Firewall", "FirewallPolicyArn"]).to_string(),
    )
}

/// One row of the change-protection refusal table.
#[derive(Debug)]
struct ProtectedCase {
    /// The firewall the call is addressed to.
    firewall: &'static str,
    /// The operation the setting refuses.
    operation: &'static str,
    request: Value,
    /// The whole message the refusal must carry.
    expected_message: &'static str,
    /// Strings the request carries that must NOT appear in the message.
    absent: Vec<String>,
}

/// RULE: a change one of a firewall's change-protection settings blocks is
/// refused with HTTP 400 / `InvalidOperationException` whose message is
/// `Cannot <Operation> because <Member> is on` — the operation being served and
/// the `Firewall` member carrying the setting, and nothing else from the
/// request — and the firewall is left exactly as it was.
///
/// The rows vary what the rule generalises over: which setting is on, which
/// operation it refuses, how the firewall is identified (name or ARN), how much
/// the request asks to change, and whether more than one setting is on at once.
#[tokio::test]
async fn a_change_protection_refusal_names_the_call_and_the_setting() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("guard-policy").await;
    let other_policy_arn = harness.shared_policy("guard-policy-other").await;

    create_firewall(
        &harness,
        "subnet-locked-fw",
        &policy_arn,
        &["subnet-c01"],
        Protections {
            subnet_change: true,
            ..Protections::default()
        },
    )
    .await;
    create_firewall(
        &harness,
        "policy-locked-fw",
        &policy_arn,
        &["subnet-c02"],
        Protections {
            policy_change: true,
            ..Protections::default()
        },
    )
    .await;
    create_firewall(
        &harness,
        "both-locked-fw",
        &policy_arn,
        &["subnet-c03"],
        Protections {
            delete: true,
            subnet_change: true,
            policy_change: true,
        },
    )
    .await;

    let cases = [
        ProtectedCase {
            firewall: "subnet-locked-fw",
            operation: "AssociateSubnets",
            request: json!({
                "FirewallName": "subnet-locked-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-c99" }],
            }),
            expected_message: "Cannot AssociateSubnets because SubnetChangeProtection is on",
            absent: vec!["subnet-locked-fw".to_string(), "subnet-c99".to_string()],
        },
        ProtectedCase {
            firewall: "subnet-locked-fw",
            operation: "AssociateSubnets",
            request: json!({
                "FirewallArn": firewall_arn("subnet-locked-fw"),
                "SubnetMappings": [
                    { "SubnetId": "subnet-c97" },
                    { "SubnetId": "subnet-c96", "IPAddressType": "IPV4" },
                    { "SubnetId": "subnet-c95", "IPAddressType": "DUALSTACK" },
                ],
            }),
            expected_message: "Cannot AssociateSubnets because SubnetChangeProtection is on",
            absent: vec![
                firewall_arn("subnet-locked-fw"),
                ACCOUNT.to_string(),
                "subnet-c97".to_string(),
            ],
        },
        ProtectedCase {
            firewall: "policy-locked-fw",
            operation: "AssociateFirewallPolicy",
            request: json!({
                "FirewallName": "policy-locked-fw",
                "FirewallPolicyArn": other_policy_arn.clone(),
            }),
            expected_message:
                "Cannot AssociateFirewallPolicy because FirewallPolicyChangeProtection is on",
            absent: vec!["policy-locked-fw".to_string(), other_policy_arn.clone()],
        },
        ProtectedCase {
            firewall: "both-locked-fw",
            operation: "AssociateFirewallPolicy",
            request: json!({
                "FirewallArn": firewall_arn("both-locked-fw"),
                "FirewallPolicyArn": other_policy_arn.clone(),
            }),
            expected_message:
                "Cannot AssociateFirewallPolicy because FirewallPolicyChangeProtection is on",
            absent: vec![firewall_arn("both-locked-fw"), other_policy_arn.clone()],
        },
        ProtectedCase {
            firewall: "both-locked-fw",
            operation: "AssociateSubnets",
            request: json!({
                "FirewallName": "both-locked-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-c94" }],
            }),
            expected_message: "Cannot AssociateSubnets because SubnetChangeProtection is on",
            absent: vec!["both-locked-fw".to_string(), "subnet-c94".to_string()],
        },
    ];

    for case in &cases {
        let before = described_state(&harness, case.firewall).await;

        let (status, body) = harness.call(case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidOperationException",
            "case {case:?}: expected error code InvalidOperationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, case.expected_message,
            "case {case:?}: expected message {:?}, got {message:?}",
            case.expected_message
        );
        for value in &case.absent {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: expected the refusal to carry nothing from the request, but \
                 {message:?} contains {value:?}"
            );
        }

        let after = described_state(&harness, case.firewall).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave the firewall untouched, got {after:?}"
        );
    }
}

/// One row of the protection-setting toggle table.
#[derive(Debug)]
struct ToggleCase {
    /// The operation that modifies the setting.
    operation: &'static str,
    /// The input and output member it carries the setting in, which is also the
    /// `Firewall` member a reader sees it on.
    member: &'static str,
    firewall: &'static str,
    /// The value the firewall is created with.
    created_with: bool,
}

/// RULE: an operation that modifies one of a firewall's protection settings
/// stores the value it was given, echoes it back with the token the resource
/// then carries, and leaves that value for any later reader — and the token
/// moves exactly when the stored value did.
///
/// The rows are the two settings this service serves a modify operation for, each
/// driven from both starting values through the whole value set of a boolean.
#[tokio::test]
async fn a_protection_setting_is_stored_and_read_back() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("toggle-policy").await;

    let cases = [
        ToggleCase {
            operation: "UpdateFirewallDeleteProtection",
            member: "DeleteProtection",
            firewall: "delete-guard-on-fw",
            created_with: true,
        },
        ToggleCase {
            operation: "UpdateFirewallDeleteProtection",
            member: "DeleteProtection",
            firewall: "delete-guard-off-fw",
            created_with: false,
        },
        ToggleCase {
            operation: "UpdateSubnetChangeProtection",
            member: "SubnetChangeProtection",
            firewall: "subnet-guard-on-fw",
            created_with: true,
        },
        ToggleCase {
            operation: "UpdateSubnetChangeProtection",
            member: "SubnetChangeProtection",
            firewall: "subnet-guard-off-fw",
            created_with: false,
        },
    ];

    for case in &cases {
        let protections = Protections {
            delete: case.member == "DeleteProtection" && case.created_with,
            subnet_change: case.member == "SubnetChangeProtection" && case.created_with,
            policy_change: false,
        };
        create_firewall(
            &harness,
            case.firewall,
            &policy_arn,
            &["subnet-d01"],
            protections,
        )
        .await;
        assert_eq!(
            described_flag(&harness, case.firewall, case.member).await,
            case.created_with,
            "case {case:?}: expected the created firewall to report the setting it was created \
             with"
        );

        // Every value, including the one the firewall already carries: the
        // request is honoured, and the token moves only where the stored value
        // did.
        let requested = [!case.created_with, !case.created_with, case.created_with];
        let mut token = described_state(&harness, case.firewall).await.0;
        let mut stored = case.created_with;
        for value in requested {
            let updated = harness
                .ok(
                    case.operation,
                    &json!({ "FirewallName": case.firewall, case.member: value }),
                )
                .await;

            assert_eq!(
                updated[case.member].as_bool(),
                Some(value),
                "case {case:?} setting {value}: expected the response to echo the stored \
                 setting, got {updated:?}"
            );
            assert_eq!(
                string_at(&updated, &["FirewallName"]),
                case.firewall,
                "case {case:?} setting {value}: expected the response to identify the firewall \
                 by name, got {updated:?}"
            );
            assert_eq!(
                string_at(&updated, &["FirewallArn"]),
                firewall_arn(case.firewall),
                "case {case:?} setting {value}: expected the response to identify the firewall \
                 by ARN, got {updated:?}"
            );

            let next = string_at(&updated, &["UpdateToken"]).to_string();
            assert!(
                is_uuid_form(&next),
                "case {case:?} setting {value}: expected an UpdateToken in the modelled UUID \
                 form, got {next:?}"
            );
            assert_eq!(
                next != token,
                stored != value,
                "case {case:?} setting {value}: expected the UpdateToken to move exactly when \
                 the stored setting did (before {token:?}, after {next:?})"
            );
            token = next;
            stored = value;

            // The response is not a per-call projection: a later reader sees the
            // same value, and the token the resource carries is the one handed
            // back.
            let read_back = described_flag(&harness, case.firewall, case.member).await;
            assert_eq!(
                read_back, value,
                "case {case:?} setting {value}: expected DescribeFirewall to report the stored \
                 setting, got {read_back}"
            );
            assert_eq!(
                described_state(&harness, case.firewall).await.0,
                token,
                "case {case:?} setting {value}: expected the firewall to carry the token the \
                 update handed back"
            );
        }
    }
}

/// RULE: the guarded call honours the STORED setting — once a modify operation
/// has turned a setting off the call it guarded goes through, and once it is back
/// on the same call is refused again.
#[tokio::test]
async fn the_stored_setting_governs_the_guarded_call() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("governs-policy").await;

    // Subnet changes: protected, then unprotected, then protected again.
    create_firewall(
        &harness,
        "flip-subnet-fw",
        &policy_arn,
        &["subnet-e01"],
        Protections {
            subnet_change: true,
            ..Protections::default()
        },
    )
    .await;
    let associate = json!({
        "FirewallName": "flip-subnet-fw",
        "SubnetMappings": [{ "SubnetId": "subnet-e02" }],
    });

    let (status, body) = harness.call("AssociateSubnets", &associate).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case protected: expected the subnet change to be refused, got {status} {body:?}"
    );

    harness
        .ok(
            "UpdateSubnetChangeProtection",
            &json!({ "FirewallName": "flip-subnet-fw", "SubnetChangeProtection": false }),
        )
        .await;
    let associated = harness.ok("AssociateSubnets", &associate).await;
    let subnets: Vec<&str> = associated["SubnetMappings"]
        .as_array()
        .unwrap_or_else(|| panic!("expected SubnetMappings in {associated:?}"))
        .iter()
        .map(|mapping| string_at(mapping, &["SubnetId"]))
        .collect();
    assert_eq!(
        subnets,
        vec!["subnet-e01", "subnet-e02"],
        "case unprotected: expected the identical call to associate the subnet, got \
         {associated:?}"
    );

    harness
        .ok(
            "UpdateSubnetChangeProtection",
            &json!({ "FirewallName": "flip-subnet-fw", "SubnetChangeProtection": true }),
        )
        .await;
    let (status, body) = harness
        .call(
            "AssociateSubnets",
            &json!({
                "FirewallName": "flip-subnet-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-e03" }],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case protected again: expected the subnet change to be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_message(&body),
        "Cannot AssociateSubnets because SubnetChangeProtection is on",
        "case protected again: expected the refusal to name the call and the setting, got \
         {body:?}"
    );

    // Deletion: unprotected, then protected, then unprotected again.
    create_firewall(
        &harness,
        "flip-delete-fw",
        &policy_arn,
        &["subnet-e04"],
        Protections::default(),
    )
    .await;
    harness
        .ok(
            "UpdateFirewallDeleteProtection",
            &json!({ "FirewallName": "flip-delete-fw", "DeleteProtection": true }),
        )
        .await;
    let (status, body) = harness
        .call("DeleteFirewall", &json!({ "FirewallName": "flip-delete-fw" }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case delete protected: expected the delete to be refused, got {status} {body:?}"
    );
    assert!(
        described_flag(&harness, "flip-delete-fw", "DeleteProtection").await,
        "case delete protected: expected the firewall to survive the refused delete, still \
         protected"
    );

    harness
        .ok(
            "UpdateFirewallDeleteProtection",
            &json!({ "FirewallName": "flip-delete-fw", "DeleteProtection": false }),
        )
        .await;
    let deleted = harness
        .ok("DeleteFirewall", &json!({ "FirewallName": "flip-delete-fw" }))
        .await;
    assert_eq!(
        deleted["Firewall"]["DeleteProtection"],
        json!(false),
        "case delete unprotected: expected the delete to go through, got {deleted:?}"
    );
}

/// RULE: a name or ARN that names no stored firewall locates no resource, so
/// every call naming one — a read or a mutation — is refused with HTTP 400 /
/// `ResourceNotFoundException` carrying this service's one fixed not-found text.
///
/// The rows span both: the firewall a `DeleteFirewall` has removed and one that
/// was never created, each named by ARN and by name, across the reads and the
/// mutations the service serves on a firewall.
#[tokio::test]
async fn a_call_on_a_gone_firewall_is_refused_as_not_found() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("gone-policy").await;
    create_firewall(
        &harness,
        "gone-fw",
        &policy_arn,
        &["subnet-f01"],
        Protections::default(),
    )
    .await;
    harness
        .ok("DeleteFirewall", &json!({ "FirewallName": "gone-fw" }))
        .await;

    // Both ways of naming a firewall, for one that is gone and one that never
    // existed.
    let identifiers = [
        ("the deleted firewall by ARN", json!({ "FirewallArn": firewall_arn("gone-fw") })),
        ("the deleted firewall by name", json!({ "FirewallName": "gone-fw" })),
        (
            "a never-created firewall by ARN",
            json!({ "FirewallArn": firewall_arn("never-made-fw") }),
        ),
        (
            "a never-created firewall by name",
            json!({ "FirewallName": "never-made-fw" }),
        ),
    ];
    // The reads and the mutations this service serves on a firewall, each with
    // whatever members it requires beside the identifier.
    let operations = [
        ("DescribeFirewall", json!({})),
        ("UpdateFirewallDeleteProtection", json!({ "DeleteProtection": false })),
        ("UpdateSubnetChangeProtection", json!({ "SubnetChangeProtection": false })),
        ("UpdateFirewallDescription", json!({ "Description": "no such firewall" })),
        (
            "AssociateSubnets",
            json!({ "SubnetMappings": [{ "SubnetId": "subnet-f02" }] }),
        ),
        ("AssociateFirewallPolicy", json!({ "FirewallPolicyArn": policy_arn })),
        ("DeleteFirewall", json!({})),
    ];

    for (identifier_label, identifier) in &identifiers {
        for (operation, extra) in &operations {
            let mut request = identifier.clone();
            for (member, value) in extra.as_object().expect("an object of extra members") {
                request[member] = value.clone();
            }

            let (status, body) = harness.call(operation, &request).await;
            let case = format!("{operation} on {identifier_label}");
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with body {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case}: expected error code ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                NOT_FOUND_MESSAGE,
                "case {case}: expected the fixed not-found message, got {body:?}"
            );
        }
    }
}

/// RULE: `UpdateToken` is the firewall's optimistic lock on the
/// protection-setting operations too — a token that is not the one the resource
/// carries is refused with `InvalidTokenException`, the setting is left as it
/// was, and the token the resource does carry is accepted.
#[tokio::test]
async fn a_stale_token_cannot_change_a_protection_setting() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("token-guard-policy").await;

    let cases = [
        ("UpdateFirewallDeleteProtection", "DeleteProtection", "token-delete-fw"),
        (
            "UpdateSubnetChangeProtection",
            "SubnetChangeProtection",
            "token-subnet-fw",
        ),
    ];

    for (operation, member, firewall) in cases {
        create_firewall(
            &harness,
            firewall,
            &policy_arn,
            &["subnet-g01"],
            Protections::default(),
        )
        .await;

        let (status, body) = harness
            .call(
                operation,
                &json!({
                    "FirewallName": firewall,
                    "UpdateToken": "00000000-0000-0000-0000-000000000000",
                    member: true,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {operation} with a stale token: expected HTTP 400, got {status} with body \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidTokenException",
            "case {operation} with a stale token: expected InvalidTokenException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            "Update token isn't valid.",
            "case {operation} with a stale token: expected the fixed token message, got {body:?}"
        );
        assert!(
            !described_flag(&harness, firewall, member).await,
            "case {operation} with a stale token: expected the refusal to leave {member} as it \
             was"
        );

        // The token the resource carries is accepted, and the same token is then
        // stale for the next conditional change.
        let token = described_state(&harness, firewall).await.0;
        let updated = harness
            .ok(
                operation,
                &json!({ "FirewallName": firewall, "UpdateToken": token, member: true }),
            )
            .await;
        assert_eq!(
            updated[member].as_bool(),
            Some(true),
            "case {operation} with the current token: expected the change to be accepted, got \
             {updated:?}"
        );
        let (status, body) = harness
            .call(
                operation,
                &json!({ "FirewallName": firewall, "UpdateToken": token, member: false }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidTokenException"),
            "case {operation} replaying the superseded token: expected it to be refused, got \
             {status} {body:?}"
        );
        assert!(
            described_flag(&harness, firewall, member).await,
            "case {operation} replaying the superseded token: expected {member} to keep the \
             accepted value"
        );
    }
}
