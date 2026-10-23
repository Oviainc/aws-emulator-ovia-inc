// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) RuleGroup permission policy, end to end over the
//! wire: `CreateRuleGroup`, `PutPermissionPolicy`, `GetPermissionPolicy`.
//!
//! Every call goes through the real router (the shared harness in `tests/all/wire.rs`),
//! so what these tests assert is what an SDK client sees: the policy a read reports is
//! the one that was PERSISTED by an earlier write, byte for byte, never an echo of the
//! reading request.
//!
//! The rule these tables span: a permission policy is accepted exactly when the document
//! satisfies the restrictions `PutPermissionPolicy` documents (IAM policy version
//! 2012-10-17; every statement carrying `Effect` `Allow`, a `Principal` and only the
//! documented `Action`s, and no `Resource`) and the `ResourceArn` names a `RuleGroup`
//! this service holds; anything else is refused and stores nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    PERMISSION_POLICY_ACTIONS, PERMISSION_POLICY_VERSION,
};

use crate::wire::{call, change_token, expect_error, ok_call, router};

/// The region the wire harness signs for, and so the one the ARNs below name.
const REGION: &str = crate::wire::DEFAULT_TEST_REGION;

/// Create a rule group and answer its `RuleGroupId`.
async fn rule_group_id(router: &MultiServiceRouter, name: &str, metric_name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRuleGroup",
        &json!({"Name": name, "MetricName": metric_name, "ChangeToken": token}),
    )
    .await;
    body["RuleGroup"]["RuleGroupId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRuleGroup returned no RuleGroupId: {body}"))
        .to_string()
}

/// The ARN AWS WAF Classic's regional endpoints give a rule group:
/// `arn:aws:waf-regional:<region>:<account>:rulegroup/<RuleGroupId>`. The account
/// segment is the emulator's canonical one, taken from the engine rather than written
/// out.
fn rule_group_arn(rule_group_id: &str) -> String {
    format!(
        "arn:aws:waf-regional:{REGION}:{}:rulegroup/{rule_group_id}",
        engine::emulator_account_id()
    )
}

/// A policy document granting `action` (one action or a list) to one AWS principal,
/// rendered as compact JSON text.
fn policy_granting(action: Value) -> String {
    json!({
        "Version": PERMISSION_POLICY_VERSION,
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
            "Action": action,
        }],
    })
    .to_string()
}

/// The policy `GetPermissionPolicy` reports for `arn`.
async fn get_policy(router: &MultiServiceRouter, arn: &str) -> String {
    let body = ok_call(router, "GetPermissionPolicy", &json!({"ResourceArn": arn})).await;
    body["Policy"]
        .as_str()
        .unwrap_or_else(|| panic!("GetPermissionPolicy returned no Policy: {body}"))
        .to_string()
}

/// A policy inside the documented restrictions is attached to the rule group its ARN
/// names, and `GetPermissionPolicy` reads back the stored document — across a table of
/// documents differing in statement form, action count, principal form and whitespace.
#[tokio::test]
async fn put_then_get_reads_back_the_stored_policy() {
    let account = engine::emulator_account_id();
    let every_action = policy_granting(json!(PERMISSION_POLICY_ACTIONS));
    let cases: Vec<(&str, String)> = vec![
        (
            "one action as a string",
            policy_granting(json!("waf-regional:GetRuleGroup")),
        ),
        (
            "one action as a single-element list",
            policy_granting(json!(["waf-regional:GetRuleGroup"])),
        ),
        ("every documented action", every_action),
        (
            "a bare statement object rather than a list",
            json!({
                "Version": PERMISSION_POLICY_VERSION,
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                    "Action": "waf:GetRuleGroup",
                },
            })
            .to_string(),
        ),
        (
            "two statements, each within the restrictions",
            json!({
                "Version": PERMISSION_POLICY_VERSION,
                "Statement": [
                    {
                        "Sid": "share-read",
                        "Effect": "Allow",
                        "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                        "Action": "waf-regional:GetRuleGroup",
                    },
                    {
                        "Sid": "share-update",
                        "Effect": "Allow",
                        "Principal": {"AWS": [format!("arn:aws:iam::{account}:root")]},
                        "Action": ["waf-regional:UpdateWebACL"],
                    },
                ],
            })
            .to_string(),
        ),
        (
            "a principal given as a bare string",
            json!({
                "Version": PERMISSION_POLICY_VERSION,
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "waf:UpdateWebACL",
                }],
            })
            .to_string(),
        ),
        (
            "indented text with newlines, which must be stored byte for byte",
            format!(
                "{{\n  \"Version\": \"{PERMISSION_POLICY_VERSION}\",\n  \"Statement\": [\n    \
                 {{\n      \"Effect\": \"Allow\",\n      \"Principal\": {{\"AWS\": \
                 \"arn:aws:iam::{account}:root\"}},\n      \"Action\": \
                 \"waf-regional:GetRuleGroup\"\n    }}\n  ]\n}}\n"
            ),
        ),
    ];

    let router = router();
    let id = rule_group_id(&router, "policy-roundtrip", "policyRoundtrip").await;
    let arn = rule_group_arn(&id);

    for (label, policy) in &cases {
        let response = ok_call(
            &router,
            "PutPermissionPolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
        )
        .await;
        assert_eq!(
            response,
            json!({}),
            "case {label:?}: PutPermissionPolicy models an empty response, got {response}"
        );

        // The round trip: the document that was persisted is what a reader sees, with no
        // re-serialization of the caller's text.
        let stored = get_policy(&router, &arn).await;
        assert_eq!(
            &stored, policy,
            "case {label:?}: expected the stored policy {policy:?}, got {stored:?}"
        );
        // A second read is identical: reading changes nothing a later caller sees.
        let reread = get_policy(&router, &arn).await;
        assert_eq!(
            reread, stored,
            "case {label:?}: a second GetPermissionPolicy must report the same policy, \
             got {reread:?}"
        );
        // The rule group itself is untouched by the policy: the modelled `RuleGroup`
        // carries no policy member.
        let group = ok_call(&router, "GetRuleGroup", &json!({"RuleGroupId": id})).await;
        assert_eq!(
            group["RuleGroup"]["RuleGroupId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the rule group must still read back, got {group}"
        );
    }
}

/// The documented action catalog, entry by entry: every action
/// `PERMISSION_POLICY_ACTIONS` names is accepted on its own (source:
/// `PutPermissionPolicy`'s own documentation, through the crate's constant, so every
/// entry is covered — including any this test does not spell out).
#[tokio::test]
async fn every_documented_action_is_accepted_on_its_own() {
    for action in PERMISSION_POLICY_ACTIONS {
        let router = router();
        let id = rule_group_id(&router, "one-action", "oneAction").await;
        let arn = rule_group_arn(&id);
        let policy = policy_granting(json!(action));
        ok_call(
            &router,
            "PutPermissionPolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
        )
        .await;
        let stored = get_policy(&router, &arn).await;
        assert_eq!(
            stored, policy,
            "case {action:?}: a documented action must be accepted and stored, got {stored:?}"
        );
    }
}

/// A policy outside the documented restrictions is refused with the modelled
/// `WAFInvalidPermissionPolicyException`, and the refusal stores nothing: the policy
/// attached before it is still the one a reader sees.
#[tokio::test]
async fn put_refuses_a_policy_outside_the_documented_restrictions() {
    let account = engine::emulator_account_id();
    let principal = json!({"AWS": format!("arn:aws:iam::{account}:root")});
    // Each row is one restriction `PutPermissionPolicy` publishes, broken on its own.
    let statement = |members: Value| {
        json!({"Version": PERMISSION_POLICY_VERSION, "Statement": [members]}).to_string()
    };
    let cases: Vec<(&str, String)> = vec![
        ("not JSON at all", "this-is-not-a-policy".to_string()),
        (
            "a JSON array rather than a policy object",
            json!([{"Effect": "Allow"}]).to_string(),
        ),
        ("a JSON string", json!("Allow").to_string()),
        (
            "an older IAM policy version",
            json!({
                "Version": "2008-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": principal,
                    "Action": "waf:GetRuleGroup",
                }],
            })
            .to_string(),
        ),
        (
            "no Version at all",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": principal,
                    "Action": "waf:GetRuleGroup",
                }],
            })
            .to_string(),
        ),
        (
            "no Statement",
            json!({"Version": PERMISSION_POLICY_VERSION}).to_string(),
        ),
        (
            "an empty Statement list",
            json!({"Version": PERMISSION_POLICY_VERSION, "Statement": []}).to_string(),
        ),
        (
            "an Effect of Deny",
            statement(json!({
                "Effect": "Deny",
                "Principal": principal,
                "Action": "waf:GetRuleGroup",
            })),
        ),
        (
            "no Effect",
            statement(json!({"Principal": principal, "Action": "waf:GetRuleGroup"})),
        ),
        (
            "no Principal",
            statement(json!({"Effect": "Allow", "Action": "waf:GetRuleGroup"})),
        ),
        (
            "an empty Principal map",
            statement(json!({
                "Effect": "Allow",
                "Principal": {},
                "Action": "waf:GetRuleGroup",
            })),
        ),
        (
            "a NotPrincipal in place of a Principal",
            statement(json!({
                "Effect": "Allow",
                "NotPrincipal": principal,
                "Action": "waf:GetRuleGroup",
            })),
        ),
        (
            "no Action",
            statement(json!({"Effect": "Allow", "Principal": principal})),
        ),
        (
            "an empty Action list",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": [],
            })),
        ),
        (
            "a wildcard action",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": "*",
            })),
        ),
        (
            "a service-wide wildcard action",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": "waf-regional:*",
            })),
        ),
        (
            "an action outside the documented list",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": "waf-regional:CreateRule",
            })),
        ),
        (
            "a documented action beside an extra one",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": ["waf-regional:GetRuleGroup", "waf-regional:DeleteRuleGroup"],
            })),
        ),
        (
            "a NotAction in place of an Action",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "NotAction": "waf-regional:CreateRule",
            })),
        ),
        (
            "a Resource parameter",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": "waf-regional:GetRuleGroup",
                "Resource": "*",
            })),
        ),
        (
            "a NotResource parameter",
            statement(json!({
                "Effect": "Allow",
                "Principal": principal,
                "Action": "waf-regional:GetRuleGroup",
                "NotResource": "*",
            })),
        ),
        (
            "one statement within the restrictions and one outside them",
            json!({
                "Version": PERMISSION_POLICY_VERSION,
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": principal,
                        "Action": "waf-regional:GetRuleGroup",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": principal,
                        "Action": "waf-regional:GetRuleGroup",
                        "Resource": "*",
                    },
                ],
            })
            .to_string(),
        ),
    ];

    let router = router();
    let id = rule_group_id(&router, "refused-policy", "refusedPolicy").await;
    let arn = rule_group_arn(&id);
    // The policy that IS attached, so every refusal below can be shown to store nothing.
    let attached = policy_granting(json!("waf-regional:GetRuleGroup"));
    ok_call(
        &router,
        "PutPermissionPolicy",
        &json!({"ResourceArn": arn, "Policy": attached}),
    )
    .await;

    for (label, policy) in &cases {
        expect_error(
            &router,
            "PutPermissionPolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
            "WAFInvalidPermissionPolicyException",
            label,
        )
        .await;
        let stored = get_policy(&router, &arn).await;
        assert_eq!(
            stored, attached,
            "case {label:?}: a refused put must leave the attached policy as it was, \
             got {stored:?}"
        );
    }
}

/// "The ARN in the request must be a valid WAF RuleGroup ARN": an ARN of another shape
/// or another resource kind is the modelled `WAFInvalidPermissionPolicyException`, while
/// a well-formed rule-group ARN this service holds nothing under is the modelled
/// `WAFNonexistentItemException`.
#[tokio::test]
async fn put_refuses_an_arn_that_names_no_rule_group_of_this_service() {
    let router = router();
    let id = rule_group_id(&router, "arn-shapes", "arnShapes").await;
    let account = engine::emulator_account_id();
    let policy = policy_granting(json!("waf-regional:GetRuleGroup"));

    // Rows differing in resource kind, service, field count and id: none of them is a
    // waf-regional rule-group ARN.
    let not_a_rule_group_arn: Vec<(&str, String)> = vec![
        ("no ARN syntax at all", "rulegroup/anything".to_string()),
        (
            "a web ACL ARN",
            format!("arn:aws:waf-regional:{REGION}:{account}:webacl/{id}"),
        ),
        (
            "a rule ARN",
            format!("arn:aws:waf-regional:{REGION}:{account}:rule/{id}"),
        ),
        (
            "a rate-based rule ARN",
            format!("arn:aws:waf-regional:{REGION}:{account}:ratebasedrule/{id}"),
        ),
        (
            "the global (waf) service rather than waf-regional",
            format!("arn:aws:waf:{REGION}:{account}:rulegroup/{id}"),
        ),
        (
            "no resource id after the type",
            format!("arn:aws:waf-regional:{REGION}:{account}:rulegroup/"),
        ),
        (
            "no resource type before the id",
            format!("arn:aws:waf-regional:{REGION}:{account}:{id}"),
        ),
        (
            "a seventh colon-separated field",
            format!("arn:aws:waf-regional:{REGION}:{account}:rulegroup/{id}:1"),
        ),
        (
            "one field short of an ARN",
            format!("arn:aws:waf-regional:{account}:rulegroup/{id}"),
        ),
    ];
    for (label, arn) in &not_a_rule_group_arn {
        expect_error(
            &router,
            "PutPermissionPolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
            "WAFInvalidPermissionPolicyException",
            label,
        )
        .await;
    }

    // A rule-group ARN of exactly the right shape, naming an id nothing was stored
    // under: the referenced object doesn't exist.
    let last = id.chars().next_back().unwrap_or('0');
    let near_miss = format!("{}{}", &id[..id.len() - 1], if last == 'a' { 'b' } else { 'a' });
    let unknown: Vec<(&str, String)> = vec![
        (
            "a UUID-shaped id nothing was stored under",
            rule_group_arn("00000000-0000-0000-0000-000000000000"),
        ),
        ("one character (the shortest id)", rule_group_arn("x")),
        ("one character off a stored id", rule_group_arn(&near_miss)),
        (
            "an unknown id in another region's ARN",
            format!(
                "arn:aws:waf-regional:eu-west-3:{account}:rulegroup/\
                 11111111-2222-4333-8444-555555555555"
            ),
        ),
    ];
    for (label, arn) in &unknown {
        expect_error(
            &router,
            "PutPermissionPolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        // Nothing was attached, so a read of the same ARN is a not-found too.
        expect_error(
            &router,
            "GetPermissionPolicy",
            &json!({"ResourceArn": arn}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
    }

    // The refusals attached nothing to the rule group that DOES exist.
    expect_error(
        &router,
        "GetPermissionPolicy",
        &json!({"ResourceArn": rule_group_arn(&id)}),
        "WAFNonexistentItemException",
        "the stored rule group after the refusals",
    )
    .await;
}

/// `GetPermissionPolicy` reports a policy only where one is attached: a rule group that
/// never had one, and an ARN naming no rule group, are both the modelled
/// `WAFNonexistentItemException` — and the refusal carries one request-independent
/// sentence that never echoes the ARN.
#[tokio::test]
async fn get_reports_no_policy_until_one_is_attached() {
    let router = router();
    let id = rule_group_id(&router, "unattached", "unattached").await;
    let arn = rule_group_arn(&id);

    let body = expect_error(
        &router,
        "GetPermissionPolicy",
        &json!({"ResourceArn": arn}),
        "WAFNonexistentItemException",
        "a rule group with no policy attached",
    )
    .await;
    let message = crate::wire::error_message(&body);
    assert!(
        !message.contains(&id) && !message.contains(&arn),
        "the refusal must not echo the requested ARN, got {message:?}"
    );

    // Attaching makes the read succeed; nothing else about the read changed.
    let policy = policy_granting(json!("waf:GetRuleGroup"));
    ok_call(
        &router,
        "PutPermissionPolicy",
        &json!({"ResourceArn": arn, "Policy": policy}),
    )
    .await;
    let stored = get_policy(&router, &arn).await;
    assert_eq!(
        stored, policy,
        "after a put the read must report the stored policy, got {stored:?}"
    );

    // A second rule group is its own resource: the policy attached to the first one is
    // not readable through the second one's ARN.
    let other = rule_group_id(&router, "unattached-2", "unattached2").await;
    expect_error(
        &router,
        "GetPermissionPolicy",
        &json!({"ResourceArn": rule_group_arn(&other)}),
        "WAFNonexistentItemException",
        "another rule group's ARN",
    )
    .await;
}

/// "You can attach only one policy with each PutPermissionPolicy request": a rule group
/// carries exactly one policy, so a later put REPLACES the stored document — across a
/// sequence of documents differing in action set and formatting.
#[tokio::test]
async fn attaching_again_replaces_the_stored_policy() {
    let router = router();
    let id = rule_group_id(&router, "replace", "replace").await;
    let arn = rule_group_arn(&id);

    let sequence = [
        ("the first policy", policy_granting(json!("waf:GetRuleGroup"))),
        (
            "a policy granting two actions",
            policy_granting(json!(["waf:GetRuleGroup", "waf:UpdateWebACL"])),
        ),
        (
            "back to one action, differently formatted",
            policy_granting(json!(["waf-regional:GetRuleGroup"])),
        ),
    ];

    let mut previous: Option<String> = None;
    for (label, policy) in &sequence {
        ok_call(
            &router,
            "PutPermissionPolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
        )
        .await;
        let stored = get_policy(&router, &arn).await;
        assert_eq!(
            &stored, policy,
            "case {label:?}: the last put must be the stored policy, got {stored:?}"
        );
        if let Some(previous) = &previous {
            assert_ne!(
                &stored, previous,
                "case {label:?}: the replaced policy must no longer be stored, got {stored:?}"
            );
        }
        previous = Some(stored);
    }

    // Exactly one policy is stored: the read reports a single document, not a list.
    let body = ok_call(&router, "GetPermissionPolicy", &json!({"ResourceArn": arn})).await;
    assert!(
        body["Policy"].is_string(),
        "GetPermissionPolicy must report one policy document, got {body}"
    );
}

/// A policy at the edges of the modelled `PolicyString` length: the model bounds it at
/// `len 1..395000`, and a document inside that bound is stored whatever its size, while
/// one past it is refused before the handler runs.
#[tokio::test]
async fn policy_length_bounds_are_honoured() {
    let router = router();
    let id = rule_group_id(&router, "sized", "sized").await;
    let arn = rule_group_arn(&id);

    // A valid policy padded with a long `Sid` so the document approaches the modelled
    // maximum of 395000 characters.
    let long_sid = "s".repeat(390_000);
    let padded = json!({
        "Version": PERMISSION_POLICY_VERSION,
        "Statement": [{
            "Sid": long_sid,
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
            "Action": "waf-regional:GetRuleGroup",
        }],
    })
    .to_string();
    assert!(
        padded.len() < 395_000,
        "the padded policy must stay inside the modelled maximum, got {}",
        padded.len()
    );
    ok_call(
        &router,
        "PutPermissionPolicy",
        &json!({"ResourceArn": arn, "Policy": padded}),
    )
    .await;
    let stored = get_policy(&router, &arn).await;
    assert_eq!(
        stored.len(),
        padded.len(),
        "a long policy must be stored whole, got {} characters",
        stored.len()
    );

    // One character past the modelled maximum is outside the model, so the request is
    // refused before this handler ever runs.
    let too_long = "p".repeat(395_001);
    let (status, body) = call(
        &router,
        "PutPermissionPolicy",
        &json!({"ResourceArn": arn, "Policy": too_long}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the modelled maximum: expected 400, got {status}: {body}"
    );
    let stored = get_policy(&router, &arn).await;
    assert_eq!(
        stored, padded,
        "a refused put must leave the attached policy as it was, got {} characters",
        stored.len()
    );
}
