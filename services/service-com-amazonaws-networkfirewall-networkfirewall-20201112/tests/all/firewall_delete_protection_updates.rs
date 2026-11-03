// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateFirewallDeleteProtection`, the operation that
//! modifies the firewall flag `DeleteProtection`.
//!
//! `tests/all/firewall_protection_settings.rs` pins the value round trip and the
//! token refusal this operation shares with the other protection-setting
//! updates. These tests pin the three rules that suite does not reach:
//!
//! * the firewall the call changes is resolved from `FirewallName` alone, from
//!   `FirewallArn` alone, or from both together ("You must specify the ARN or
//!   the name, and you can specify both" — `UpdateFirewallDeleteProtection$
//!   FirewallArn`), and a request naming neither, or naming two different
//!   firewalls, is refused with `InvalidRequestException` (HTTP 400) before
//!   anything is written;
//! * `UpdateToken` is OPTIONAL and the lock is conditional on it being sent:
//!   "To make an unconditional change to the firewall, omit the token in your
//!   update request. Without the token, Network Firewall performs your updates
//!   regardless of whether the firewall has changed since you last retrieved
//!   it" (`UpdateFirewallDeleteProtection$UpdateToken`);
//! * the value this call stores is the one `DeleteFirewall` then honours, which
//!   "requires the firewall's DeleteProtection flag to be FALSE"
//!   (`DeleteFirewall`).
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`.

use serde_json::{json, Value};

use crate::harness::{
    error_code, error_message, is_uuid_form, string_at, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// The firewall ARN this emulator builds for `name`.
fn firewall_arn(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{name}")
}

/// Create a firewall on `policy_arn` carrying `delete_protection`.
async fn create_firewall(
    harness: &Harness,
    name: &str,
    policy_arn: &str,
    subnet: &str,
    delete_protection: bool,
) -> Value {
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": subnet }],
                "DeleteProtection": delete_protection,
            }),
        )
        .await
}

/// The firewall `name`'s stored `DeleteProtection` and `UpdateToken`, read back
/// through `DescribeFirewall`.
async fn described(harness: &Harness, name: &str) -> (bool, String) {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    (
        described["Firewall"]["DeleteProtection"]
            .as_bool()
            .unwrap_or_else(|| panic!("expected Firewall.DeleteProtection in {described:?}")),
        string_at(&described, &["UpdateToken"]).to_string(),
    )
}

/// One row of the identification table: how a request names the firewall it
/// changes.
#[derive(Debug)]
struct IdentifierCase {
    label: &'static str,
    firewall: &'static str,
    subnet: &'static str,
    /// Whether the firewall is created delete-protected.
    created_with: bool,
    /// The identifier members the update and the delete carry.
    identifier: Value,
}

/// RULE: a firewall operation that takes `FirewallName` and `FirewallArn`
/// identifies its firewall from either member alone or from both together, so
/// `UpdateFirewallDeleteProtection` stores the requested flag on the SAME
/// firewall whichever form the request uses — and the stored flag is what a
/// later `DescribeFirewall` reports and what `DeleteFirewall` honours.
///
/// The rows vary the identification form (name, ARN, both) and the starting
/// value of the flag, and each row drives the flag to the value that makes the
/// delete succeed or be refused, so both outcomes are observed per form.
#[tokio::test]
async fn the_delete_protection_update_resolves_the_firewall_by_name_or_arn() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("ident-policy").await;

    let cases = [
        IdentifierCase {
            label: "by name only",
            firewall: "ident-name-fw",
            subnet: "subnet-h01",
            created_with: false,
            identifier: json!({ "FirewallName": "ident-name-fw" }),
        },
        IdentifierCase {
            label: "by ARN only",
            firewall: "ident-arn-fw",
            subnet: "subnet-h02",
            created_with: false,
            identifier: json!({ "FirewallArn": firewall_arn("ident-arn-fw") }),
        },
        IdentifierCase {
            label: "by name and ARN together",
            firewall: "ident-both-fw",
            subnet: "subnet-h03",
            created_with: true,
            identifier: json!({
                "FirewallName": "ident-both-fw",
                "FirewallArn": firewall_arn("ident-both-fw"),
            }),
        },
        IdentifierCase {
            label: "by ARN only, created protected",
            firewall: "ident-arn-protected-fw",
            subnet: "subnet-h04",
            created_with: true,
            identifier: json!({ "FirewallArn": firewall_arn("ident-arn-protected-fw") }),
        },
    ];

    for case in &cases {
        create_firewall(
            &harness,
            case.firewall,
            &policy_arn,
            case.subnet,
            case.created_with,
        )
        .await;

        // Turn the flag ON through this identification form: the response
        // reports the firewall by BOTH members whichever one the request used.
        let mut request = case.identifier.clone();
        request["DeleteProtection"] = json!(true);
        let updated = harness
            .ok("UpdateFirewallDeleteProtection", &request)
            .await;
        assert_eq!(
            updated["DeleteProtection"].as_bool(),
            Some(true),
            "case {} ({case:?}): expected the response to echo the stored flag, got {updated:?}",
            case.label
        );
        assert_eq!(
            string_at(&updated, &["FirewallName"]),
            case.firewall,
            "case {case:?}: expected the response to name the resolved firewall, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, &["FirewallArn"]),
            firewall_arn(case.firewall),
            "case {case:?}: expected the response to carry the resolved firewall's ARN, got \
             {updated:?}"
        );
        let token = string_at(&updated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&token),
            "case {case:?}: expected an UpdateToken in the modelled UUID form, got {token:?}"
        );

        let (stored, described_token) = described(&harness, case.firewall).await;
        assert!(
            stored,
            "case {case:?}: expected DescribeFirewall to report the stored flag as on"
        );
        assert_eq!(
            described_token, token,
            "case {case:?}: expected the firewall to carry the token the update handed back"
        );

        // The STORED flag is what the delete honours, through the same
        // identification form.
        let (status, body) = harness.call("DeleteFirewall", &case.identifier).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {case:?}: expected the protected firewall's delete to be refused, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_message(&body),
            "Unable to delete the object because delete protection is enabled",
            "case {case:?}: expected the delete-protected refusal text, got {body:?}"
        );

        // Turn it back off through the same form, and the identical delete goes
        // through.
        let mut request = case.identifier.clone();
        request["DeleteProtection"] = json!(false);
        let updated = harness
            .ok("UpdateFirewallDeleteProtection", &request)
            .await;
        assert_eq!(
            updated["DeleteProtection"].as_bool(),
            Some(false),
            "case {case:?}: expected the response to echo the cleared flag, got {updated:?}"
        );
        let (cleared, _) = described(&harness, case.firewall).await;
        assert!(
            !cleared,
            "case {case:?}: expected DescribeFirewall to report the stored flag as off"
        );

        let deleted = harness.ok("DeleteFirewall", &case.identifier).await;
        assert_eq!(
            deleted["Firewall"]["DeleteProtection"],
            json!(false),
            "case {case:?}: expected the unprotected delete to go through, got {deleted:?}"
        );
    }
}

/// One row of the bad-identifier table.
#[derive(Debug)]
struct BadIdentifierCase {
    label: &'static str,
    request: Value,
    expected_code: &'static str,
    expected_message: String,
}

/// RULE: a request whose name/ARN pair does not identify exactly one stored
/// firewall is refused before anything is written — naming neither member is
/// `InvalidRequestException` (HTTP 400) naming the two members, a pair whose two
/// members identify different firewalls is `InvalidRequestException` reporting
/// that disagreement; naming one that is not stored is
/// `ResourceNotFoundException` (HTTP 400) with this service's one fixed
/// not-found text.
///
/// The rows span the three ways the pair can fail, with the valid flag value
/// present in every one of them so the refusal is about the identifier alone.
#[tokio::test]
async fn a_delete_protection_update_that_identifies_no_one_firewall_is_refused() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("bad-ident-policy").await;
    create_firewall(&harness, "left-fw", &policy_arn, "subnet-i01", false).await;
    create_firewall(&harness, "right-fw", &policy_arn, "subnet-i02", false).await;

    let cases = [
        BadIdentifierCase {
            label: "neither name nor ARN",
            request: json!({ "DeleteProtection": true }),
            expected_code: "InvalidRequestException",
            expected_message: "At least one of FirewallName or FirewallArn must be set".to_string(),
        },
        BadIdentifierCase {
            label: "name and ARN identify different firewalls",
            request: json!({
                "FirewallName": "left-fw",
                "FirewallArn": firewall_arn("right-fw"),
                "DeleteProtection": true,
            }),
            expected_code: "InvalidRequestException",
            expected_message: "Name and Arn(Name in Arn) do not match".to_string(),
        },
        BadIdentifierCase {
            label: "an ARN no stored firewall carries",
            request: json!({
                "FirewallArn": firewall_arn("absent-fw"),
                "DeleteProtection": true,
            }),
            expected_code: "ResourceNotFoundException",
            expected_message: NOT_FOUND_MESSAGE.to_string(),
        },
    ];

    for case in &cases {
        let (status, body) = harness
            .call("UpdateFirewallDeleteProtection", &case.request)
            .await;
        assert_eq!(
            status, 400,
            "case {} ({case:?}): expected HTTP 400, got {status} with body {body:?}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            case.expected_code,
            "case {case:?}: expected error code {}, got {body:?}",
            case.expected_code
        );
        assert_eq!(
            error_message(&body),
            case.expected_message,
            "case {case:?}: expected message {:?}, got {body:?}",
            case.expected_message
        );

        // Nothing was written: both firewalls keep the flag and the token they
        // carried.
        for firewall in ["left-fw", "right-fw"] {
            let (stored, _) = described(&harness, firewall).await;
            assert!(
                !stored,
                "case {case:?}: expected the refusal to leave {firewall}'s DeleteProtection as it \
                 was"
            );
        }
    }
}

/// RULE: `UpdateToken` is optional, and omitting it makes the change
/// UNCONDITIONAL — the update is performed regardless of whether the firewall
/// changed since the caller last retrieved it — while sending one makes it
/// conditional on that token still being the firewall's current one.
///
/// The firewall's token is deliberately moved by intervening updates between
/// each read and each write, which is exactly the condition that distinguishes
/// the two forms.
#[tokio::test]
async fn an_omitted_token_changes_delete_protection_unconditionally() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("unconditional-policy").await;
    create_firewall(&harness, "unconditional-fw", &policy_arn, "subnet-j01", false).await;

    // The token the caller "last retrieved", then two intervening changes that
    // move it, so the retrieved token is stale by the time the caller writes.
    let (_, retrieved) = described(&harness, "unconditional-fw").await;
    for value in [true, false] {
        harness
            .ok(
                "UpdateFirewallDescription",
                &json!({ "FirewallName": "unconditional-fw", "Description": format!("drift {value}") }),
            )
            .await;
    }
    let (_, moved) = described(&harness, "unconditional-fw").await;
    assert_ne!(
        moved, retrieved,
        "case setup: expected the intervening updates to move the firewall's UpdateToken"
    );

    // Conditional on the stale token: refused, nothing written.
    let (status, body) = harness
        .call(
            "UpdateFirewallDeleteProtection",
            &json!({
                "FirewallName": "unconditional-fw",
                "UpdateToken": retrieved,
                "DeleteProtection": true,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidTokenException"),
        "case stale token: expected the conditional change to be refused, got {status} {body:?}"
    );
    let (stored, _) = described(&harness, "unconditional-fw").await;
    assert!(
        !stored,
        "case stale token: expected the refusal to leave DeleteProtection as it was"
    );

    // Unconditional: the SAME change with the token omitted is performed,
    // whatever the firewall's current token is.
    let updated = harness
        .ok(
            "UpdateFirewallDeleteProtection",
            &json!({ "FirewallName": "unconditional-fw", "DeleteProtection": true }),
        )
        .await;
    assert_eq!(
        updated["DeleteProtection"].as_bool(),
        Some(true),
        "case omitted token: expected the unconditional change to be performed, got {updated:?}"
    );
    let (stored, after) = described(&harness, "unconditional-fw").await;
    assert!(
        stored,
        "case omitted token: expected DescribeFirewall to report the unconditionally stored flag"
    );
    assert_ne!(
        after, moved,
        "case omitted token: expected the accepted change to re-mint the firewall's UpdateToken"
    );

    // And it stays unconditional after the token moved again: the caller never
    // has to re-read.
    harness
        .ok(
            "UpdateFirewallDescription",
            &json!({ "FirewallName": "unconditional-fw", "Description": "drift again" }),
        )
        .await;
    let updated = harness
        .ok(
            "UpdateFirewallDeleteProtection",
            &json!({ "FirewallName": "unconditional-fw", "DeleteProtection": false }),
        )
        .await;
    assert_eq!(
        updated["DeleteProtection"].as_bool(),
        Some(false),
        "case omitted token after a further change: expected the change to be performed, got \
         {updated:?}"
    );
    let (stored, _) = described(&harness, "unconditional-fw").await;
    assert!(
        !stored,
        "case omitted token after a further change: expected the stored flag to be off"
    );
}
