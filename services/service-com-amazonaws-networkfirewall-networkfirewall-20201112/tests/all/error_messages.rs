// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's error MESSAGES.
//!
//! Network Firewall's exception text is a property of the EXCEPTION, not of the
//! request: one constant string answers every `ResourceNotFoundException`
//! whatever resource was asked for, one answers every name already taken, one
//! answers every stale `UpdateToken` — and where a refusal IS built from the
//! request it takes a single parseable shape, a fixed stem followed by the
//! offending value in `, parameter: [...]` brackets.
//!
//! Each test below is a TABLE whose rows span the class the rule covers:
//! different operations, different resource kinds, different names and ARNs, the
//! read path and the write path. A row's identifiers are deliberately distinct so
//! that a message leaking request-derived content fails on more than equality.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, minimal_policy_document, resource_policy_document, string_at, Harness, ACCOUNT,
    REGION,
};

/// The text every `ResourceNotFoundException` carries, as recorded off the
/// service: byte-identical across five operations in three fixtures, naming
/// neither the resource, nor its type, nor its ARN.
const NOT_FOUND: &str = "No object with the provided name or resource identifier was found";

/// The text a create whose name is already taken is refused with, as recorded.
const ALREADY_EXISTS: &str = "A resource with the specified name already exists";

/// The text every `InvalidTokenException` carries, as recorded.
const INVALID_TOKEN: &str = "Update token isn't valid.";

/// The `Message` member of an error body.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// A rule-group ARN in this emulator's form, for a rule group that was never
/// created.
fn absent_rule_group_arn(group_type: &str, name: &str) -> String {
    let kind = if group_type == "STATELESS" {
        "stateless-rulegroup"
    } else {
        "stateful-rulegroup"
    };
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:{kind}/{name}")
}

/// A `STATEFUL` rule group document carrying one pass rule.
fn stateful_document() -> Value {
    json!({ "RulesSource": { "StatefulRules": [{
        "Action": "PASS",
        "Header": {
            "Protocol": "TCP",
            "Source": "ANY",
            "SourcePort": "ANY",
            "Direction": "ANY",
            "Destination": "ANY",
            "DestinationPort": "ANY",
        },
        "RuleOptions": [{ "Keyword": "sid", "Settings": ["1"] }],
    }]}})
}

/// A `STATELESS` rule group document carrying one pass rule.
fn stateless_document() -> Value {
    json!({ "RulesSource": { "StatelessRulesAndCustomActions": {
        "StatelessRules": [{
            "Priority": 1,
            "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
        }],
    }}})
}

/// Create a firewall on a fresh policy and return its name.
async fn firewall_on_new_policy(harness: &Harness, name: &str) -> String {
    let policy_arn = harness.shared_policy(&format!("{name}-policy")).await;
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": format!("subnet-{name}") }],
            }),
        )
        .await;
    name.to_string()
}

/// RULE: every `ResourceNotFoundException` this service answers carries ONE
/// constant message, identical across operations and resource kinds and carrying
/// nothing from the request.
///
/// Rows span the resource kinds that can be missed (firewall, firewall policy,
/// stateless and stateful rule group, resource policy), the read and the write
/// path for each, and identification by name as well as by ARN.
#[tokio::test]
async fn resource_not_found_carries_one_constant_message() {
    let harness = Harness::new();
    // An existing rule group with NO resource policy attached: asking for its
    // policy misses the policy, not the rule group.
    let unshared_arn = harness
        .shared_rule_group("policyless-group", "STATEFUL", 10)
        .await;

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyName": "absent-policy-by-name" }),
            "absent-policy-by-name",
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyArn":
                format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/absent-policy-by-arn") }),
            "absent-policy-by-arn",
        ),
        (
            "DeleteFirewallPolicy",
            json!({ "FirewallPolicyName": "absent-policy-to-delete" }),
            "absent-policy-to-delete",
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "absent-stateful-group", "Type": "STATEFUL" }),
            "absent-stateful-group",
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "absent-stateless-group", "Type": "STATELESS" }),
            "absent-stateless-group",
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupArn": absent_rule_group_arn("STATEFUL", "absent-group-by-arn") }),
            "absent-group-by-arn",
        ),
        (
            "DeleteRuleGroup",
            json!({ "RuleGroupName": "absent-group-to-delete", "Type": "STATELESS" }),
            "absent-group-to-delete",
        ),
        (
            "UpdateRuleGroup",
            json!({
                "RuleGroupName": "absent-group-to-update",
                "Type": "STATEFUL",
                "UpdateToken": state::new_uuid(),
                "RuleGroup": stateful_document(),
            }),
            "absent-group-to-update",
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "absent-firewall" }),
            "absent-firewall",
        ),
        (
            "DeleteFirewall",
            json!({ "FirewallArn":
                format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/absent-firewall-by-arn") }),
            "absent-firewall-by-arn",
        ),
        (
            "UpdateFirewallDescription",
            json!({ "FirewallName": "absent-firewall-to-edit", "Description": "x" }),
            "absent-firewall-to-edit",
        ),
        // The cause the corpus records: an existing resource that carries no
        // resource policy. The ARN must not reach the message.
        (
            "DescribeResourcePolicy",
            json!({ "ResourceArn": unshared_arn.clone() }),
            "policyless-group",
        ),
        (
            "DeleteResourcePolicy",
            json!({ "ResourceArn": unshared_arn.clone() }),
            "policyless-group",
        ),
        (
            "DescribeResourcePolicy",
            json!({ "ResourceArn": absent_rule_group_arn("STATELESS", "absent-shared-group") }),
            "absent-shared-group",
        ),
        (
            "PutResourcePolicy",
            json!({
                "ResourceArn": absent_rule_group_arn("STATEFUL", "absent-share-target"),
                "Policy": resource_policy_document(
                    &state::RULE_GROUP_POLICY_ACTIONS,
                    &absent_rule_group_arn("STATEFUL", "absent-share-target"),
                ),
            }),
            "absent-share-target",
        ),
    ];

    for (target, request, leaked) in &rows {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {target} {request:?}: expected 400 ResourceNotFoundException, \
             got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, NOT_FOUND,
            "case {target} {request:?}: expected the constant not-found message {NOT_FOUND:?}, \
             got {message:?}"
        );
        assert!(
            !message.contains(leaked),
            "case {target} {request:?}: the message must carry no request-derived content, \
             but {message:?} names {leaked:?}"
        );
    }
}

/// RULE: a create whose name is already taken is refused with ONE constant
/// message that names neither the kind of resource, nor its type, nor the
/// requested name.
///
/// Rows span the three resources this service creates by name, and both halves of
/// the rule-group namespace (a name is taken only within its `Type`).
#[tokio::test]
async fn a_name_already_taken_is_refused_with_one_constant_message() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("shared-dup-policy").await;

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "dup-stateful-group",
                "Type": "STATEFUL",
                "Capacity": 10,
                "RuleGroup": stateful_document(),
            }),
            "dup-stateful-group",
        ),
        (
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "dup-stateless-group",
                "Type": "STATELESS",
                "Capacity": 10,
                "RuleGroup": stateless_document(),
            }),
            "dup-stateless-group",
        ),
        (
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "dup-firewall-policy",
                "FirewallPolicy": minimal_policy_document(),
            }),
            "dup-firewall-policy",
        ),
        (
            "CreateFirewall",
            json!({
                "FirewallName": "dup-firewall",
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-dup-firewall" }],
            }),
            "dup-firewall",
        ),
    ];

    for (target, request, name) in &rows {
        // The first create succeeds; the SECOND, byte-identical one collides.
        harness.ok(target, request).await;
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            status, 400,
            "case {target} {request:?}: expected the repeat create to be refused with 400, \
             got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, ALREADY_EXISTS,
            "case {target} {request:?}: expected the constant already-exists message \
             {ALREADY_EXISTS:?}, got {message:?}"
        );
        assert!(
            !message.contains(name),
            "case {target} {request:?}: the message must carry no request-derived content, \
             but {message:?} names {name:?}"
        );
        for leaked in ["STATEFUL", "STATELESS", "firewall policy", "rule group"] {
            assert!(
                !message.contains(leaked),
                "case {target} {request:?}: the message must be resource-agnostic, \
                 but {message:?} names {leaked:?}"
            );
        }
    }
}

/// RULE: every `InvalidTokenException` this service answers carries ONE constant
/// message, naming neither the resource nor the token that was rejected.
///
/// Rows span both resources that carry an optimistic-locking token and the ways a
/// token can be wrong: superseded by a later write, never minted at all, and
/// minted for a DIFFERENT resource.
#[tokio::test]
async fn a_rejected_update_token_carries_one_constant_message() {
    let harness = Harness::new();
    harness.shared_rule_group("token-group", "STATEFUL", 10).await;
    harness
        .shared_rule_group("other-token-group", "STATEFUL", 10)
        .await;

    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "token-group", "Type": "STATEFUL" }),
        )
        .await;
    let first_token = string_at(&described, &["UpdateToken"]).to_string();
    // One accepted update rotates the token, so `first_token` is now superseded.
    harness
        .ok(
            "UpdateRuleGroup",
            &json!({
                "RuleGroupName": "token-group",
                "Type": "STATEFUL",
                "UpdateToken": first_token.clone(),
                "Description": "rotated",
            }),
        )
        .await;
    let other = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "other-token-group", "Type": "STATEFUL" }),
        )
        .await;
    let other_token = string_at(&other, &["UpdateToken"]).to_string();

    let firewall = firewall_on_new_policy(&harness, "token-firewall").await;

    let rows: Vec<(&str, Value)> = vec![
        // Superseded by the update above.
        (
            "UpdateRuleGroup",
            json!({
                "RuleGroupName": "token-group",
                "Type": "STATEFUL",
                "UpdateToken": first_token.clone(),
                "Description": "stale",
            }),
        ),
        // Never minted for anything.
        (
            "UpdateRuleGroup",
            json!({
                "RuleGroupName": "token-group",
                "Type": "STATEFUL",
                "UpdateToken": state::new_uuid(),
                "Description": "invented",
            }),
        ),
        // Minted for a different rule group.
        (
            "UpdateRuleGroup",
            json!({
                "RuleGroupName": "token-group",
                "Type": "STATEFUL",
                "UpdateToken": other_token.clone(),
                "Description": "borrowed",
            }),
        ),
        // The same rule on a different resource.
        (
            "UpdateFirewallDescription",
            json!({
                "FirewallName": firewall,
                "UpdateToken": state::new_uuid(),
                "Description": "invented",
            }),
        ),
    ];

    for (target, request) in &rows {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidTokenException"),
            "case {target} {request:?}: expected 400 InvalidTokenException, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, INVALID_TOKEN,
            "case {target} {request:?}: expected the constant token message {INVALID_TOKEN:?}, \
             got {message:?}"
        );
    }

    // The refused updates changed nothing: the rule group still carries the
    // description the one accepted update left.
    let after = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "token-group", "Type": "STATEFUL" }),
        )
        .await;
    assert_eq!(
        after["RuleGroupResponse"]["Description"],
        json!("rotated"),
        "a refused update must leave the rule group as the last accepted write left it, \
         got {after:?}"
    );
}

/// RULE: a refusal whose message IS built from the request takes one parseable
/// shape — a fixed stem naming the referenced resource KIND and the defect, then
/// the offending value in `, parameter: [...]` brackets — and never names the
/// input member the value arrived in.
///
/// Rows span both reference lists of a firewall policy document, a firewall's
/// policy reference, stateless and stateful rule-group ARNs, and ARNs for names
/// and regions the emulator never minted.
#[tokio::test]
async fn a_reference_to_an_absent_arn_names_the_kind_and_brackets_the_arn() {
    let harness = Harness::new();

    let rows: Vec<(&str, Value, String, String)> = vec![
        (
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "ref-stateful-missing",
                "FirewallPolicy": {
                    "StatelessDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                    "StatefulRuleGroupReferences": [
                        { "ResourceArn": absent_rule_group_arn("STATEFUL", "nope") },
                    ],
                },
            }),
            "rule group ARN does not exist".to_string(),
            absent_rule_group_arn("STATEFUL", "nope"),
        ),
        (
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "ref-stateless-missing",
                "FirewallPolicy": {
                    "StatelessDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessRuleGroupReferences": [
                        { "ResourceArn": absent_rule_group_arn("STATELESS", "also-nope"),
                          "Priority": 1 },
                    ],
                },
            }),
            "rule group ARN does not exist".to_string(),
            absent_rule_group_arn("STATELESS", "also-nope"),
        ),
        (
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "ref-foreign-region",
                "FirewallPolicy": {
                    "StatelessDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                    "StatefulRuleGroupReferences": [
                        { "ResourceArn":
                            format!("arn:aws:network-firewall:us-west-2:{ACCOUNT}:stateful-rulegroup/elsewhere") },
                    ],
                },
            }),
            "rule group ARN does not exist".to_string(),
            format!("arn:aws:network-firewall:us-west-2:{ACCOUNT}:stateful-rulegroup/elsewhere"),
        ),
        (
            "CreateFirewall",
            json!({
                "FirewallName": "ref-missing-policy",
                "FirewallPolicyArn":
                    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/nope"),
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-ref" }],
            }),
            "firewall policy ARN does not exist".to_string(),
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/nope"),
        ),
    ];

    for (target, request, stem, arn) in &rows {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {target} {request:?}: expected 400 InvalidRequestException, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("{stem}, parameter: [{arn}]"),
            "case {target} {request:?}: expected the fixed stem plus the bracketed parameter, \
             got {message:?}"
        );
        for member in [
            "StatefulRuleGroupReferences",
            "StatelessRuleGroupReferences",
            "FirewallPolicy.",
            "FirewallPolicyArn",
        ] {
            assert!(
                !message.contains(member),
                "case {target} {request:?}: the stem must not name the input member, \
                 but {message:?} names {member:?}"
            );
        }
    }
}

/// RULE: the same request-derived shape answers a reference that names a rule
/// group of the WRONG kind for the list it sits in — a fixed stem plus the
/// offending ARN in parameter brackets.
#[tokio::test]
async fn a_reference_of_the_wrong_kind_keeps_the_bracketed_parameter_shape() {
    let harness = Harness::new();
    let stateful_arn = harness
        .shared_rule_group("kind-stateful", "STATEFUL", 10)
        .await;
    let stateless_arn = harness
        .shared_rule_group("kind-stateless", "STATELESS", 10)
        .await;

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "kind-swapped-stateless",
                "FirewallPolicy": {
                    "StatelessDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessRuleGroupReferences": [
                        { "ResourceArn": stateful_arn.clone(), "Priority": 1 },
                    ],
                },
            }),
            stateful_arn.clone(),
        ),
        (
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "kind-swapped-stateful",
                "FirewallPolicy": {
                    "StatelessDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                    "StatefulRuleGroupReferences": [
                        { "ResourceArn": stateless_arn.clone() },
                    ],
                },
            }),
            stateless_arn.clone(),
        ),
    ];

    for (target, request, arn) in &rows {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {target} {request:?}: expected 400 InvalidRequestException, got {status} {body:?}"
        );
        let message = error_message(&body);
        let suffix = format!(", parameter: [{arn}]");
        assert!(
            message.ends_with(&suffix),
            "case {target} {request:?}: expected the message to end with {suffix:?}, \
             got {message:?}"
        );
        assert!(
            !message.contains("RuleGroupReferences"),
            "case {target} {request:?}: the stem must not name the input member, got {message:?}"
        );
    }
}
