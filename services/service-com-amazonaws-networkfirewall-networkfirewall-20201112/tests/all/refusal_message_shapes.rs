// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SHAPE of this service's refusal messages on the
//! delete, capacity and resource-policy paths.
//!
//! Two rules, both properties of the CONDITION rather than of the request:
//!
//! * a refusal the service answers the same way whatever was asked for carries
//!   ONE constant message with no request-derived content — the delete refusals
//!   (`InvalidOperationException`: protected, still in use), the
//!   policy-validation refusal (`InvalidResourcePolicyException`) and the
//!   not-found answer (`ResourceNotFoundException`);
//! * a refusal that IS built from the request takes the service's one parseable
//!   built shape, `<stem>, parameter: [<value>], context: <input path>` — the
//!   capacity check on both the create and the update path, whose bracketed value
//!   is the capacity the submitted rules REQUIRE.
//!
//! The constants below are written out as literals rather than read from
//! `state`, so these tests pin the wire text instead of restating the code.
//!
//! Each test is a TABLE whose rows span the class its rule covers: the resource
//! kinds a delete can refuse, every `RulesSource` member rules can arrive in and
//! the units they need, both shareable resource types and every way a policy
//! document can fail. Every assertion names its row and the actual value.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, error_message, resource_policy_document, string_at, Harness, ACCOUNT,
    NOT_FOUND_MESSAGE, REGION,
};

/// The text a delete refused by delete protection carries, as recorded off the
/// service: resource-agnostic, naming "the object" and nothing from the request.
const DELETE_PROTECTED: &str = "Unable to delete the object because delete protection is enabled";

/// The text a delete refused because the object is still referenced carries — the
/// same constant family as [`DELETE_PROTECTED`].
const DELETE_IN_USE: &str = "Unable to delete the object because it is still in use";

/// The text every `InvalidResourcePolicyException` carries, as recorded: the
/// submitted document did not match the RAM managed permission, and the refusal
/// says only that.
const INVALID_RESOURCE_POLICY: &str = "The supplied policy does not match RAM managed permissions.";

/// One `StatefulRules` entry carrying the given Suricata `sid`.
fn stateful_rule(sid: u32) -> Value {
    json!({
        "Action": "PASS",
        "Header": {
            "Protocol": "TCP",
            "Source": "ANY",
            "SourcePort": "ANY",
            "Direction": "ANY",
            "Destination": "ANY",
            "DestinationPort": "ANY",
        },
        "RuleOptions": [{ "Keyword": "sid", "Settings": [sid.to_string()] }],
    })
}

/// A `StatefulRules` document carrying `count` individual rules, which is exactly
/// the capacity a stateful rule group needs for it.
fn stateful_rules_document(count: u32) -> Value {
    let rules: Vec<Value> = (1..=count).map(stateful_rule).collect();
    json!({ "RulesSource": { "StatefulRules": rules } })
}

/// A `RulesString` document carrying `count` Suricata rule lines.
fn rules_string_document(count: u32) -> Value {
    let lines: Vec<String> = (1..=count)
        .map(|sid| format!("pass tcp any any -> any any (sid:{sid};)"))
        .collect();
    json!({ "RulesSource": { "RulesString": lines.join("\n") } })
}

/// A `RulesSourceList` document carrying `count` domain name specifications.
fn rules_source_list_document(count: u32) -> Value {
    let targets: Vec<String> = (1..=count).map(|index| format!(".host{index}.test")).collect();
    json!({ "RulesSource": { "RulesSourceList": {
        "Targets": targets,
        "TargetTypes": ["TLS_SNI"],
        "GeneratedRulesType": "ALLOWLIST",
    }}})
}

/// A `StatelessRulesAndCustomActions` document whose single rule needs
/// `sources * destinations` capacity units (the documented product over its match
/// settings, an absent setting counting as one).
fn stateless_document(sources: u32, destinations: u32) -> Value {
    let addresses = |count: u32, octet: u32| -> Vec<Value> {
        (0..count)
            .map(|index| json!({ "AddressDefinition": format!("10.{octet}.{index}.0/24") }))
            .collect()
    };
    let mut attributes = json!({});
    if sources > 1 {
        attributes["Sources"] = json!(addresses(sources, 0));
    }
    if destinations > 1 {
        attributes["Destinations"] = json!(addresses(destinations, 1));
    }
    json!({ "RulesSource": { "StatelessRulesAndCustomActions": {
        "StatelessRules": [{
            "Priority": 1,
            "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": attributes },
        }],
    }}})
}

/// Create a firewall on `policy_arn` and return its ARN.
async fn create_firewall(
    harness: &Harness,
    name: &str,
    policy_arn: &str,
    delete_protection: bool,
) -> String {
    let created = harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": format!("subnet-{name}") }],
                "DeleteProtection": delete_protection,
            }),
        )
        .await;
    string_at(&created, &["Firewall", "FirewallArn"]).to_string()
}

/// RULE: a delete the service refuses because the object is protected against
/// deletion, or because something still references it, is answered with
/// `InvalidOperationException` (HTTP 400) carrying the one constant message for
/// that CONDITION — identical across resource kinds and carrying neither the
/// resource's name or ARN, nor its type, nor how many references hold it.
///
/// Rows span both conditions, two resource kinds, identification by name and by
/// ARN, two differently named protected firewalls and reference counts of one and
/// of three, so a count or a name in the text fails on more than equality.
#[tokio::test]
async fn a_refused_delete_carries_the_constant_for_its_condition() {
    let harness = Harness::new();

    // Two delete-protected firewalls with deliberately different names.
    let protected_policy = harness.shared_policy("protected-fw-policy").await;
    let protected_arn = create_firewall(&harness, "protected-fw", &protected_policy, true).await;
    create_firewall(&harness, "guarded-prod-edge", &protected_policy, true).await;

    // A firewall policy one firewall uses, and another three firewalls use.
    let used_once = harness.shared_policy("used-once-policy").await;
    create_firewall(&harness, "user-of-used-once", &used_once, false).await;
    let used_thrice = harness.shared_policy("used-thrice-policy").await;
    for index in 1..=3 {
        create_firewall(
            &harness,
            &format!("user-{index}-of-used-thrice"),
            &used_thrice,
            false,
        )
        .await;
    }

    // (operation, request, expected constant, strings the message must not carry)
    let rows: Vec<(&str, Value, &str, Vec<String>)> = vec![
        (
            "DeleteFirewall",
            json!({ "FirewallName": "protected-fw" }),
            DELETE_PROTECTED,
            vec![
                "protected-fw".to_string(),
                protected_arn.clone(),
                "DeleteProtection".to_string(),
            ],
        ),
        (
            "DeleteFirewall",
            json!({ "FirewallArn": protected_arn.clone() }),
            DELETE_PROTECTED,
            vec!["protected-fw".to_string(), protected_arn.clone()],
        ),
        (
            "DeleteFirewall",
            json!({ "FirewallName": "guarded-prod-edge" }),
            DELETE_PROTECTED,
            vec!["guarded-prod-edge".to_string()],
        ),
        (
            "DeleteFirewallPolicy",
            json!({ "FirewallPolicyName": "used-once-policy" }),
            DELETE_IN_USE,
            vec![
                "used-once-policy".to_string(),
                used_once.clone(),
                "1".to_string(),
                "firewall".to_string(),
            ],
        ),
        (
            "DeleteFirewallPolicy",
            json!({ "FirewallPolicyArn": used_thrice.clone() }),
            DELETE_IN_USE,
            vec![
                "used-thrice-policy".to_string(),
                used_thrice.clone(),
                "3".to_string(),
            ],
        ),
    ];

    for (operation, request, expected, forbidden) in &rows {
        let (status, body) = harness.call(operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {operation} {request:?}: expected 400 InvalidOperationException, \
             got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            &message, expected,
            "case {operation} {request:?}: expected the constant {expected:?}, got {message:?}"
        );
        for leaked in forbidden {
            assert!(
                !message.contains(leaked.as_str()),
                "case {operation} {request:?}: the message must carry no request-derived \
                 content, but {message:?} names {leaked:?}"
            );
        }
    }

    // Every refusal above left its object in place, so each is still readable.
    let survivors: [(&str, Value); 3] = [
        ("DescribeFirewall", json!({ "FirewallName": "protected-fw" })),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "guarded-prod-edge" }),
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyName": "used-once-policy" }),
        ),
    ];
    for (operation, request) in &survivors {
        harness.ok(operation, request).await;
    }

    // The positive control for the in-use condition: once the last firewall that
    // uses the policy is gone, the same delete succeeds — the refusal is the
    // reference, not a blanket ban.
    harness
        .ok(
            "DeleteFirewall",
            &json!({ "FirewallName": "user-of-used-once" }),
        )
        .await;
    harness
        .ok(
            "DeleteFirewallPolicy",
            &json!({ "FirewallPolicyName": "used-once-policy" }),
        )
        .await;
}

/// RULE: a delete the service ACCEPTS removes the object, and a later read of it
/// is answered with the service's one constant `ResourceNotFoundException`
/// message — the same text a read of something never created gets, with nothing
/// from the request in it.
///
/// Rows span a firewall that was deleted, a firewall that never existed, and the
/// same pair resolved by ARN rather than by name.
#[tokio::test]
async fn a_firewall_that_is_gone_reports_the_constant_not_found_message() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("deletable-fw-policy").await;
    let deleted_arn = create_firewall(&harness, "deletable-fw", &policy_arn, false).await;
    harness
        .ok("DeleteFirewall", &json!({ "FirewallName": "deletable-fw" }))
        .await;

    let never_arn = format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/never-made");
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "deleted-by-name",
            json!({ "FirewallName": "deletable-fw" }),
            "deletable-fw".to_string(),
        ),
        (
            "deleted-by-arn",
            json!({ "FirewallArn": deleted_arn.clone() }),
            deleted_arn.clone(),
        ),
        (
            "never-created-by-name",
            json!({ "FirewallName": "never-made" }),
            "never-made".to_string(),
        ),
        (
            "never-created-by-arn",
            json!({ "FirewallArn": never_arn.clone() }),
            never_arn.clone(),
        ),
    ];

    for (label, request, leaked) in &rows {
        let (status, body) = harness.call("DescribeFirewall", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {label}: expected 400 ResourceNotFoundException, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {label}: expected the constant not-found message {NOT_FOUND_MESSAGE:?}, \
             got {message:?}"
        );
        assert!(
            !message.contains(leaked.as_str()),
            "case {label}: the message must carry no request-derived content, but {message:?} \
             names {leaked:?}"
        );
    }
}

/// One row of the capacity table: a rule group document, the `RulesSource` member
/// it arrives in, and the capacity the documented calculation requires for it.
struct CapacityRow {
    label: &'static str,
    group_type: &'static str,
    /// The `RulesSource` member the rules are given in, which names both the
    /// refusal's stem and its context.
    member: &'static str,
    /// A document needing `required` capacity units.
    document: Value,
    /// A document of the same source kind needing ONE unit, so a rule group can
    /// be created below `required` and then updated past it.
    seed: Value,
    /// What `CreateRuleGroupRequest$Capacity`'s calculation requires for
    /// `document`.
    required: i64,
}

/// The capacity table: one row per `RulesSource` member rules can arrive in, with
/// the units they need varying across rows.
fn capacity_rows() -> Vec<CapacityRow> {
    vec![
        CapacityRow {
            label: "two-stateful-rules",
            group_type: "STATEFUL",
            member: "StatefulRules",
            document: stateful_rules_document(2),
            seed: stateful_rules_document(1),
            required: 2,
        },
        CapacityRow {
            label: "five-stateful-rules",
            group_type: "STATEFUL",
            member: "StatefulRules",
            document: stateful_rules_document(5),
            seed: stateful_rules_document(1),
            required: 5,
        },
        CapacityRow {
            label: "three-suricata-lines",
            group_type: "STATEFUL",
            member: "RulesString",
            document: rules_string_document(3),
            seed: rules_string_document(1),
            required: 3,
        },
        CapacityRow {
            label: "four-domain-specifications",
            group_type: "STATEFUL_DOMAIN",
            member: "RulesSourceList",
            document: rules_source_list_document(4),
            seed: rules_source_list_document(1),
            required: 4,
        },
        CapacityRow {
            label: "a-stateless-rule-needing-six-units",
            group_type: "STATELESS",
            member: "StatelessRulesAndCustomActions",
            // 3 sources * 2 destinations, each other setting absent and counting
            // as one.
            document: stateless_document(3, 2),
            seed: stateless_document(1, 1),
            required: 6,
        },
    ]
}

/// The built refusal a rule group whose rules exceed the capacity available to
/// them must carry.
fn expected_capacity_message(member: &str, required: i64) -> String {
    format!("{member} capacity exceeded, parameter: [{required}], context: RulesSource.{member}")
}

/// The value a client parses out of the `, parameter: [...]` brackets, or `None`
/// when the message does not carry the built shape at all.
fn bracketed_parameter(message: &str) -> Option<&str> {
    message
        .split_once(", parameter: [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(value, _)| value)
}

/// RULE: a rule group whose rules need more capacity than is available to them is
/// refused with `InvalidRequestException` (HTTP 400) carrying the service's one
/// BUILT message shape — the stem naming the `RulesSource` member and the check,
/// the capacity those rules REQUIRE in parameter brackets, and that member's
/// input path as context — on the create path, on the dry-run path and on the
/// update path alike, where "rule group capacity is fixed at creation"
/// (`RuleGroupResponse$Capacity`) makes the required units the only number a
/// client can act on.
///
/// Rows span every `RulesSource` member rules can arrive in and required capacities
/// from 2 to 6; the capacity offered is one unit short in every refusal row, and
/// each row also checks the boundary — exactly the required capacity is accepted
/// and reported back.
#[tokio::test]
async fn rules_that_exceed_the_available_capacity_are_refused_with_the_built_message() {
    let harness = Harness::new();

    for row in capacity_rows() {
        let expected = expected_capacity_message(row.member, row.required);
        let short = row.required - 1;

        // The create path, with and without `DryRun`: "if set to TRUE, Network
        // Firewall ... checks whether the request can run successfully", so the
        // same check answers the same way.
        for (variant, dry_run) in [("create", false), ("dry-run", true)] {
            let case = format!("{}/{variant}", row.label);
            let name = format!("{}-{variant}", row.label);
            let (status, body) = harness
                .call(
                    "CreateRuleGroup",
                    &json!({
                        "RuleGroupName": name,
                        "Type": row.group_type,
                        "Capacity": short,
                        "RuleGroup": row.document,
                        "DryRun": dry_run,
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidRequestException"),
                "case {case}: expected 400 InvalidRequestException for {} units at a capacity \
                 of {short}, got {status} {body:?}",
                row.required
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case}: expected the built capacity message {expected:?}, got {message:?}"
            );
            // The point of the built shape: a client reads the required capacity
            // out of the brackets and retries with it.
            assert_eq!(
                bracketed_parameter(&message),
                Some(row.required.to_string().as_str()),
                "case {case}: expected the required capacity in the parameter brackets, \
                 got {message:?}"
            );
            // A refused create persists nothing.
            let (status, body) = harness
                .call(
                    "DescribeRuleGroup",
                    &json!({ "RuleGroupName": name, "Type": row.group_type }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: a refused create must persist nothing, got {status} {body:?}"
            );
        }

        // The boundary: exactly the required capacity is accepted, and the stored
        // rule group reports consuming it.
        let at_bound = format!("{}-at-the-bound", row.label);
        let created = harness
            .ok(
                "CreateRuleGroup",
                &json!({
                    "RuleGroupName": at_bound,
                    "Type": row.group_type,
                    "Capacity": row.required,
                    "RuleGroup": row.document,
                }),
            )
            .await;
        assert_eq!(
            created["RuleGroupResponse"]["ConsumedCapacity"],
            json!(row.required),
            "case {}/at-the-bound: expected the rule group to consume its required capacity, \
             got {created:?}",
            row.label
        );

        // The update path: a rule group created below the required capacity
        // cannot be raised to it, so the same built refusal answers an update
        // whose document needs more than the record's fixed capacity.
        let updatable = format!("{}-updatable", row.label);
        let seeded = harness
            .ok(
                "CreateRuleGroup",
                &json!({
                    "RuleGroupName": updatable,
                    "Type": row.group_type,
                    "Capacity": short,
                    "RuleGroup": row.seed,
                }),
            )
            .await;
        let token = string_at(&seeded, &["UpdateToken"]).to_string();
        let (status, body) = harness
            .call(
                "UpdateRuleGroup",
                &json!({
                    "RuleGroupName": updatable,
                    "Type": row.group_type,
                    "UpdateToken": token,
                    "RuleGroup": row.document,
                }),
            )
            .await;
        let case = format!("{}/update", row.label);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {case}: expected 400 InvalidRequestException, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case}: expected the built capacity message {expected:?}, got {message:?}"
        );
        // The refused update changed nothing: the seed document is still stored.
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": updatable, "Type": row.group_type }),
            )
            .await;
        assert_eq!(
            described["RuleGroup"], row.seed,
            "case {case}: a refused update must leave the stored document alone, \
             got {described:?}"
        );
    }
}

/// RULE: a resource policy that does not match the RAM managed permission for the
/// shared resource's type — whichever way it fails to — is refused with
/// `InvalidResourcePolicyException` (HTTP 400) carrying ONE constant message,
/// which names neither the actions the document granted, nor the ones the
/// permission allows, nor the resource it was submitted for.
///
/// Rows span both shareable resource types, an action belonging to the other
/// type's set, an action in neither set, the permitted set with one extra action,
/// and every way the document can fail to be a policy at all.
#[tokio::test]
async fn a_refused_resource_policy_carries_one_constant_message() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("message-policy").await;
    let stateful_arn = harness
        .shared_rule_group("message-stateful", "STATEFUL", 10)
        .await;
    let stateless_arn = harness
        .shared_rule_group("message-stateless", "STATELESS", 10)
        .await;

    // Documents whose granted actions are outside the resource type's permitted
    // set, per resource type.
    let out_of_set: Vec<(String, &String, String)> = vec![
        (
            "policy-granted-the-rule-group-set".to_string(),
            &policy_arn,
            resource_policy_document(&state::RULE_GROUP_POLICY_ACTIONS, &policy_arn),
        ),
        (
            // The recorded case: an action no resource type's set carries.
            "policy-granted-an-unpermitted-action".to_string(),
            &policy_arn,
            resource_policy_document(&["network-firewall:DeleteFirewallPolicy"], &policy_arn),
        ),
        (
            "policy-granted-the-set-plus-an-extra".to_string(),
            &policy_arn,
            resource_policy_document(
                &state::FIREWALL_POLICY_POLICY_ACTIONS
                    .iter()
                    .copied()
                    .chain(["network-firewall:ListRuleGroups"])
                    .collect::<Vec<&str>>(),
                &policy_arn,
            ),
        ),
        (
            "stateful-group-granted-the-firewall-policy-set".to_string(),
            &stateful_arn,
            resource_policy_document(&state::FIREWALL_POLICY_POLICY_ACTIONS, &stateful_arn),
        ),
        (
            "stateless-group-granted-an-unpermitted-action".to_string(),
            &stateless_arn,
            resource_policy_document(&["network-firewall:DeleteRuleGroup"], &stateless_arn),
        ),
    ];

    // Documents that are not a policy at all, submitted against a resource that
    // exists so the document is what is being judged.
    let permitted = state::FIREWALL_POLICY_POLICY_ACTIONS;
    let malformed: Vec<(String, &String, String)> = vec![
        ("not-json".to_string(), &policy_arn, "}{ not json".to_string()),
        (
            "json-but-not-an-object".to_string(),
            &policy_arn,
            json!([1, 2, 3]).to_string(),
        ),
        (
            "no-statement".to_string(),
            &policy_arn,
            json!({ "Version": "2012-10-17" }).to_string(),
        ),
        (
            "empty-statement-list".to_string(),
            &policy_arn,
            json!({ "Statement": [] }).to_string(),
        ),
        (
            "effect-deny".to_string(),
            &policy_arn,
            json!({ "Statement": [{
                "Effect": "Deny",
                "Principal": { "AWS": ACCOUNT },
                "Action": permitted,
            }]})
            .to_string(),
        ),
        (
            "no-principal".to_string(),
            &policy_arn,
            json!({ "Statement": [{ "Effect": "Allow", "Action": permitted }]}).to_string(),
        ),
        (
            "action-entry-that-is-not-a-string".to_string(),
            &policy_arn,
            json!({ "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": ACCOUNT },
                "Action": [7],
            }]})
            .to_string(),
        ),
    ];

    for (label, arn, document) in out_of_set.into_iter().chain(malformed) {
        let (status, body) = harness
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "Policy": document }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidResourcePolicyException"),
            "case {label}: expected 400 InvalidResourcePolicyException, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, INVALID_RESOURCE_POLICY,
            "case {label}: expected the constant {INVALID_RESOURCE_POLICY:?}, got {message:?}"
        );
        assert!(
            !message.contains("network-firewall:"),
            "case {label}: the message must name no action, got {message:?}"
        );
        assert!(
            !message.contains(arn.as_str()),
            "case {label}: the message must not quote the request's ARN, got {message:?}"
        );
        // A refused document attaches nothing.
        let (status, body) = harness
            .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {label}: a refused policy attaches nothing, got {status} {body:?}"
        );
    }

    // The positive control: a permitted subset is accepted and reads back
    // verbatim, so the refusals above are the validation rule and not a blanket
    // refusal.
    let accepted = resource_policy_document(&[state::FIREWALL_POLICY_POLICY_ACTIONS[0]], &policy_arn);
    harness
        .ok(
            "PutResourcePolicy",
            &json!({ "ResourceArn": policy_arn, "Policy": accepted }),
        )
        .await;
    let described = harness
        .ok(
            "DescribeResourcePolicy",
            &json!({ "ResourceArn": policy_arn }),
        )
        .await;
    assert_eq!(
        described["Policy"],
        json!(accepted),
        "the permitted subset must read back verbatim, got {described:?}"
    );
}
