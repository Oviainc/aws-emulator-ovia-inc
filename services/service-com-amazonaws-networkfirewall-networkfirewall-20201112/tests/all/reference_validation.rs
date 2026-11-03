// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service VALIDATES a resource reference before
//! it consults the store.
//!
//! Every operation that identifies a firewall, a firewall policy or a rule group
//! takes a name/ARN pair (plus `Type` for a rule group) in which both members are
//! optional in the model and exactly one reference must arrive. The reference is
//! judged before any lookup, so each way of getting it wrong is its own bad
//! request rather than a lookup miss:
//!
//! * neither member supplied — `InvalidRequestException` naming the two members;
//! * an ARN that is not a well-formed resource ARN — one constant;
//! * a well-formed ARN whose resource-type segment is not the type the operation
//!   addresses — one constant;
//! * a rule group named without its `Type` and without its ARN — one constant.
//!
//! Only a well-formed reference that resolves to nothing stored is
//! `ResourceNotFoundException`.
//!
//! Each test is a TABLE whose rows span the class its rule covers: all three
//! resource kinds, the read path and the write path, and — for the malformed-ARN
//! rule — every way the ARN form can break. The message constants are written out
//! as literals rather than read from `state`, so these tests pin the wire text
//! instead of restating the code.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, error_message, string_at, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// The text a reference whose ARN is not a well-formed resource ARN is refused
/// with, as recorded off the service.
const ARN_INVALID_FORMAT: &str = "Resource ARN has an invalid format.";

/// The text a reference whose ARN names another kind of resource is refused with,
/// as recorded off the service.
const ARN_TYPE_MISMATCH: &str = "Resource Type and Arn(Type in Arn) do not match";

/// The text a reference whose name and ARN name different resources is refused
/// with, as recorded off the service.
const ARN_NAME_MISMATCH: &str = "Name and Arn(Name in Arn) do not match";

/// The text a rule group referenced by name alone is refused with, as recorded
/// off the service.
const TYPE_REQUIRED: &str = "Type must be set when referring to RuleGroup by name";

/// An ARN in this service's own form for `resource_type/name`, whether or not any
/// such resource was created. The resource types are this service's own
/// (`state::ARN_TYPE_FIREWALL`, `ARN_TYPE_FIREWALL_POLICY`,
/// `ARN_TYPES_RULE_GROUP`), which is what its create operations mint.
fn arn_for(resource_type: &str, name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:{resource_type}/{name}")
}

/// Create a firewall policy and a firewall on it, and return the firewall's ARN.
async fn firewall_on_new_policy(harness: &Harness, name: &str) -> String {
    let policy_arn = harness.shared_policy(&format!("{name}-policy")).await;
    let created = harness
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
    string_at(&created, &["Firewall", "FirewallArn"]).to_string()
}

/// Assert that `target` with `request` is refused with HTTP 400
/// `InvalidRequestException` carrying exactly `message`.
async fn assert_invalid_request(harness: &Harness, target: &str, request: &Value, message: &str) {
    let (status, body) = harness.call(target, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case {target} {request:?}: expected 400 InvalidRequestException, got {status} {body:?}"
    );
    let actual = error_message(&body);
    assert_eq!(
        actual, message,
        "case {target} {request:?}: expected the message {message:?}, got {actual:?}"
    );
}

/// The refusal text of a body the FRAMEWORK rendered, before any handler ran.
///
/// A modelled error carries its text in `Message`, the member this service's
/// error shapes declare, and [`error_message`] reads that. A pre-handler refusal
/// is rendered by the framework, which lets a service choose the error class and
/// the text but not the member name it writes them under — today it writes
/// `message`. Either spelling is read here, because what the rows below assert is
/// the refusal the SERVICE chose (its class and its text), which is what it
/// controls, and they must keep asserting it when that rendering moves to the
/// modelled spelling.
fn framework_refusal_text(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// RULE: an operation whose resource is identified by a name/ARN pair refuses a
/// request that supplies NEITHER member with `InvalidRequestException`, naming
/// that operation's two member names and nothing else.
///
/// Rows span all three resource kinds, the read path, the delete path and the
/// update/associate paths, and so all three member-name pairs.
#[tokio::test]
async fn a_request_naming_neither_member_of_the_pair_is_refused() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("pairless-policy").await;

    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "DescribeFirewall",
            json!({}),
            "FirewallName",
            "FirewallArn",
        ),
        ("DeleteFirewall", json!({}), "FirewallName", "FirewallArn"),
        (
            "UpdateFirewallDescription",
            json!({ "Description": "no reference" }),
            "FirewallName",
            "FirewallArn",
        ),
        (
            "UpdateFirewallDeleteProtection",
            json!({ "DeleteProtection": true }),
            "FirewallName",
            "FirewallArn",
        ),
        (
            "UpdateSubnetChangeProtection",
            json!({ "SubnetChangeProtection": true }),
            "FirewallName",
            "FirewallArn",
        ),
        (
            "AssociateSubnets",
            json!({ "SubnetMappings": [{ "SubnetId": "subnet-0abc1" }] }),
            "FirewallName",
            "FirewallArn",
        ),
        (
            "DisassociateSubnets",
            json!({ "SubnetIds": ["subnet-0abc1"] }),
            "FirewallName",
            "FirewallArn",
        ),
        (
            "AssociateFirewallPolicy",
            json!({ "FirewallPolicyArn": policy_arn }),
            "FirewallName",
            "FirewallArn",
        ),
        (
            "DescribeFirewallPolicy",
            json!({}),
            "FirewallPolicyName",
            "FirewallPolicyArn",
        ),
        (
            "DeleteFirewallPolicy",
            json!({}),
            "FirewallPolicyName",
            "FirewallPolicyArn",
        ),
        (
            "DescribeRuleGroup",
            json!({}),
            "RuleGroupName",
            "RuleGroupArn",
        ),
        (
            "DeleteRuleGroup",
            json!({ "Type": "STATEFUL" }),
            "RuleGroupName",
            "RuleGroupArn",
        ),
        (
            "UpdateRuleGroup",
            json!({ "Type": "STATEFUL", "UpdateToken": state::new_uuid(), "Description": "x" }),
            "RuleGroupName",
            "RuleGroupArn",
        ),
    ];

    for (target, request, name_member, arn_member) in &rows {
        let expected = format!("At least one of {name_member} or {arn_member} must be set");
        assert_invalid_request(&harness, target, request, &expected).await;
    }
}

/// RULE: a reference whose ARN is not a well-formed resource ARN
/// (`arn:<partition>:<service>:<region>:<account>:<type><sep><id>`) is refused
/// with `InvalidRequestException` carrying one constant message, before the store
/// is consulted.
///
/// Rows span every way the form can break — too few segments, an empty resource
/// segment, a resource segment with no type/id separator, an empty type, an empty
/// id — across all three resource kinds and the read, delete and update paths.
#[tokio::test]
async fn an_arn_that_is_not_a_resource_arn_is_refused_as_malformed() {
    let harness = Harness::new();

    // Each value starts with `arn:aws`, which is what `ResourceArn`'s modelled
    // pattern requires, and breaks the resource-ARN form after that.
    let malformed = [
        "arn:aws",
        "arn:aws:network-firewall",
        &format!("arn:aws:network-firewall:{REGION}"),
        &format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}"),
        &format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:"),
        &format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall"),
        &format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/"),
        &format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:/named-thing"),
    ]
    .map(str::to_string);

    let paths: Vec<(&str, &str, Value)> = vec![
        ("DescribeFirewall", "FirewallArn", json!({})),
        ("DeleteFirewall", "FirewallArn", json!({})),
        (
            "UpdateFirewallDescription",
            "FirewallArn",
            json!({ "Description": "malformed reference" }),
        ),
        ("DescribeFirewallPolicy", "FirewallPolicyArn", json!({})),
        ("DeleteFirewallPolicy", "FirewallPolicyArn", json!({})),
        ("DescribeRuleGroup", "RuleGroupArn", json!({})),
        ("DeleteRuleGroup", "RuleGroupArn", json!({})),
    ];

    for (target, arn_member, extra) in &paths {
        for arn in &malformed {
            let mut request = extra.clone();
            request[arn_member] = json!(arn);
            assert_invalid_request(&harness, target, &request, ARN_INVALID_FORMAT).await;
        }
    }
}

/// RULE: a reference whose ARN is well formed but whose resource-TYPE segment is
/// not a type the operation addresses is refused with `InvalidRequestException`
/// carrying one constant message — not as a missing resource.
///
/// Rows span every ordered pair of this service's resource kinds (so each
/// operation is given each OTHER kind's ARN), an ARN of a Network Firewall
/// resource kind this operation never addresses, and an ARN of another service
/// entirely. The ARNs of the resources that EXIST are the ones the create
/// operations minted, so the refusal cannot be a lookup miss in disguise.
#[tokio::test]
async fn an_arn_of_another_resource_type_is_refused_as_a_type_mismatch() {
    let harness = Harness::new();
    let firewall_arn = firewall_on_new_policy(&harness, "mismatch-firewall").await;
    let policy_arn = harness.shared_policy("mismatch-policy").await;
    let stateful_arn = harness
        .shared_rule_group("mismatch-stateful", "STATEFUL", 10)
        .await;
    let stateless_arn = harness
        .shared_rule_group("mismatch-stateless", "STATELESS", 10)
        .await;

    let rows: Vec<(&str, &str, String, Value)> = vec![
        // A firewall operation given each other kind's ARN.
        (
            "DescribeFirewall",
            "FirewallArn",
            policy_arn.clone(),
            json!({}),
        ),
        (
            "DescribeFirewall",
            "FirewallArn",
            stateful_arn.clone(),
            json!({}),
        ),
        (
            "DeleteFirewall",
            "FirewallArn",
            stateless_arn.clone(),
            json!({}),
        ),
        (
            "UpdateFirewallDescription",
            "FirewallArn",
            policy_arn.clone(),
            json!({ "Description": "wrong kind" }),
        ),
        // A firewall-policy operation given each other kind's ARN.
        (
            "DescribeFirewallPolicy",
            "FirewallPolicyArn",
            stateful_arn.clone(),
            json!({}),
        ),
        (
            "DescribeFirewallPolicy",
            "FirewallPolicyArn",
            firewall_arn.clone(),
            json!({}),
        ),
        (
            "DeleteFirewallPolicy",
            "FirewallPolicyArn",
            stateless_arn.clone(),
            json!({}),
        ),
        // A rule-group operation given each other kind's ARN.
        (
            "DescribeRuleGroup",
            "RuleGroupArn",
            firewall_arn.clone(),
            json!({}),
        ),
        (
            "DescribeRuleGroup",
            "RuleGroupArn",
            policy_arn.clone(),
            json!({}),
        ),
        (
            "DeleteRuleGroup",
            "RuleGroupArn",
            firewall_arn.clone(),
            json!({}),
        ),
        // A Network Firewall resource kind no name/ARN pair above addresses.
        (
            "DescribeFirewall",
            "FirewallArn",
            arn_for("tls-configuration", "inspection-config"),
            json!({}),
        ),
        (
            "DescribeRuleGroup",
            "RuleGroupArn",
            arn_for("vpc-endpoint-association", "association"),
            json!({}),
        ),
        // Another service's resource entirely — including one whose own
        // resource type is spelled like one of this service's.
        (
            "DescribeFirewallPolicy",
            "FirewallPolicyArn",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role"),
            json!({}),
        ),
        (
            "DescribeFirewall",
            "FirewallArn",
            format!("arn:aws:s3:::firewall/{ACCOUNT}"),
            json!({}),
        ),
        (
            "DescribeRuleGroup",
            "RuleGroupArn",
            format!("arn:aws:wafv2:{REGION}:{ACCOUNT}:stateful-rulegroup/borrowed"),
            json!({}),
        ),
        (
            // Another vendor's resource whose resource field names no type at
            // all: the vendor already decides it, so the reference is the
            // mismatch and not a malformed ARN.
            "DescribeFirewall",
            "FirewallArn",
            "arn:aws:s3:::some-bucket".to_string(),
            json!({}),
        ),
    ];

    for (target, arn_member, arn, extra) in &rows {
        let mut request = extra.clone();
        request[arn_member] = json!(arn);
        assert_invalid_request(&harness, target, &request, ARN_TYPE_MISMATCH).await;
    }

    // A rule group's TYPE is part of its ARN, so asking for one half of the
    // rule-group name space with the other half's ARN is the same mismatch —
    // even when the two carry the same name.
    let both_halves = harness
        .shared_rule_group("mismatch-both-halves", "STATEFUL", 10)
        .await;
    assert_invalid_request(
        &harness,
        "DescribeRuleGroup",
        &json!({
            "RuleGroupName": "mismatch-both-halves",
            "Type": "STATELESS",
            "RuleGroupArn": both_halves,
        }),
        ARN_TYPE_MISMATCH,
    )
    .await;

    // The refused reads changed nothing: every resource is still readable.
    harness
        .ok("DescribeFirewall", &json!({ "FirewallArn": firewall_arn }))
        .await;
    harness
        .ok(
            "DescribeFirewallPolicy",
            &json!({ "FirewallPolicyArn": policy_arn }),
        )
        .await;
    harness
        .ok("DescribeRuleGroup", &json!({ "RuleGroupArn": stateful_arn }))
        .await;
    harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupArn": stateless_arn }),
        )
        .await;
}

/// RULE: a reference whose ARN carries nothing of the ARN form at all — so the
/// value cannot even be a resource ARN — is refused as a malformed reference too,
/// with the service's own modelled client error rather than an error shape the
/// model does not declare.
///
/// Rows span a bare name in the ARN member, an ARN-ish value with the wrong
/// scheme, a URL, and the empty-ish forms a client reaches for, over the read,
/// delete and update paths of the two resources identified by `FirewallArn` and
/// `RuleGroupArn`.
///
/// A value with no `arn:aws` prefix at all is outside the ARN member's modelled
/// `@pattern`, so the framework's own boundary check finds it before the handler
/// runs and renders the refusal BODY itself. The service chooses the error class
/// and the text, which is what these rows assert; it does not choose the member
/// the framework writes that text under (see [`framework_refusal_text`]).
/// `FirewallPolicyArn` is not among the members here: the same member name is a
/// required reference to another resource on `CreateFirewall`, whose refusal for
/// a malformed value is recorded as a validation clause
/// (`crate::validation_refusal_wording`), and this pre-handler seam sees the
/// member path without the operation.
///
/// The last two rows are outside `ResourceArn`'s declared LENGTH (1..256,
/// `smithy_query(query="shape",shape="ResourceArn")`) rather than its pattern:
/// a value that long, or empty, is as impossible an ARN as one with no `arn:`
/// scheme, so it is the same refusal.
#[tokio::test]
async fn a_value_that_is_not_an_arn_at_all_is_refused_as_malformed() {
    let harness = Harness::new();

    // 257 characters: one past `ResourceArn`'s published maximum.
    let too_long = format!(
        "arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{}",
        "n".repeat(257)
    );
    let not_arns = [
        "my-firewall".to_string(),
        "sim-probe-reference-1234".to_string(),
        "ARN:AWS:network-firewall:eu-west-1:0:firewall/x".to_string(),
        "aws:network-firewall:eu-west-1:0:firewall/x".to_string(),
        "https://network-firewall.eu-west-1.amazonaws.com/firewall/x".to_string(),
        "firewall/my-firewall".to_string(),
        too_long,
        String::new(),
    ];

    let paths: Vec<(&str, &str, Value)> = vec![
        ("DescribeFirewall", "FirewallArn", json!({})),
        ("DeleteFirewall", "FirewallArn", json!({})),
        ("DescribeRuleGroup", "RuleGroupArn", json!({})),
        ("DeleteRuleGroup", "RuleGroupArn", json!({})),
        (
            "UpdateFirewallDescription",
            "FirewallArn",
            json!({ "Description": "not an arn" }),
        ),
        (
            "UpdateSubnetChangeProtection",
            "FirewallArn",
            json!({ "SubnetChangeProtection": true }),
        ),
    ];

    for (target, arn_member, extra) in &paths {
        for value in &not_arns {
            let mut request = extra.clone();
            request[arn_member] = json!(value);
            let (status, body) = harness.call(target, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidRequestException"),
                "case {target} {request:?}: expected 400 InvalidRequestException, \
                 got {status} {body:?}"
            );
            let actual = framework_refusal_text(&body);
            assert_eq!(
                actual, ARN_INVALID_FORMAT,
                "case {target} {request:?}: expected the message \
                 {ARN_INVALID_FORMAT:?}, got {actual:?}"
            );
        }
    }
}

/// RULE: a reference that supplies BOTH a name and an ARN which identify
/// different resources is refused with `InvalidRequestException` carrying one
/// constant message.
///
/// Rows span all three resource kinds and both orders of "the stored one" and
/// "the other one", including an ARN whose name segment names nothing stored.
#[tokio::test]
async fn a_name_disagreeing_with_its_arn_is_refused() {
    let harness = Harness::new();
    let firewall_arn = firewall_on_new_policy(&harness, "paired-firewall").await;
    let other_firewall_arn = firewall_on_new_policy(&harness, "paired-other-firewall").await;
    let policy_arn = harness.shared_policy("paired-policy").await;
    let stateful_arn = harness
        .shared_rule_group("paired-stateful", "STATEFUL", 10)
        .await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "DescribeFirewall",
            json!({ "FirewallName": "paired-other-firewall", "FirewallArn": firewall_arn }),
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "paired-firewall", "FirewallArn": other_firewall_arn }),
        ),
        (
            "DeleteFirewall",
            json!({
                "FirewallName": "never-created",
                "FirewallArn": arn_for("firewall", "paired-firewall"),
            }),
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyName": "never-created", "FirewallPolicyArn": policy_arn }),
        ),
        (
            "DescribeRuleGroup",
            json!({
                "RuleGroupName": "never-created",
                "Type": "STATEFUL",
                "RuleGroupArn": stateful_arn,
            }),
        ),
        (
            // The ARN carries the type and the name, so a name beside it that
            // the stored rule group does not have is the same disagreement.
            "DescribeRuleGroup",
            json!({
                "RuleGroupName": "never-created",
                "RuleGroupArn": arn_for("stateful-rulegroup", "paired-stateful"),
            }),
        ),
    ];

    for (target, request) in &rows {
        assert_invalid_request(&harness, target, request, ARN_NAME_MISMATCH).await;
    }
}

/// RULE: a rule group referenced by `RuleGroupName` with neither `Type` nor
/// `RuleGroupArn` is refused with `InvalidRequestException` carrying one constant
/// message — a name alone names at most half of a rule group's identity.
///
/// Rows span the read, delete and update paths, a name that IS stored (in each
/// half of the rule-group name space) and one that is not: the refusal precedes
/// the lookup either way.
#[tokio::test]
async fn a_rule_group_named_without_its_type_is_refused() {
    let harness = Harness::new();
    harness
        .shared_rule_group("typeless-stateful", "STATEFUL", 10)
        .await;
    harness
        .shared_rule_group("typeless-stateless", "STATELESS", 10)
        .await;

    let names = ["typeless-stateful", "typeless-stateless", "never-created"];
    for name in names {
        let rows: Vec<(&str, Value)> = vec![
            ("DescribeRuleGroup", json!({ "RuleGroupName": name })),
            ("DeleteRuleGroup", json!({ "RuleGroupName": name })),
            (
                "UpdateRuleGroup",
                json!({
                    "RuleGroupName": name,
                    "UpdateToken": state::new_uuid(),
                    "Description": "typeless",
                }),
            ),
            (
                "DescribeRuleGroup",
                json!({ "RuleGroupName": name, "AnalyzeRuleGroup": true }),
            ),
        ];
        for (target, request) in &rows {
            assert_invalid_request(&harness, target, request, TYPE_REQUIRED).await;
        }
    }
}

/// RULE: the reference checks sit AHEAD of the lookup and replace none of it — a
/// well-formed reference of the right type still resolves to the stored resource,
/// and still misses with `ResourceNotFoundException` when nothing carries it.
///
/// Rows span all three resource kinds, identification by name and by ARN, and a
/// rule-group ARN used without `Type` (which the ARN already carries).
#[tokio::test]
async fn a_well_formed_reference_still_resolves_or_misses() {
    let harness = Harness::new();
    let firewall_arn = firewall_on_new_policy(&harness, "resolvable-firewall").await;
    let policy_arn = harness.shared_policy("resolvable-policy").await;
    let stateful_arn = harness
        .shared_rule_group("resolvable-stateful", "STATEFUL", 10)
        .await;

    // A reference that resolves: read each resource back by ARN and by name, and
    // check the resource READ is the one the reference named.
    let resolves: Vec<(&str, Value, &[&str], String)> = vec![
        (
            "DescribeFirewall",
            json!({ "FirewallArn": firewall_arn }),
            &["Firewall", "FirewallArn"],
            firewall_arn.clone(),
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "resolvable-firewall" }),
            &["Firewall", "FirewallName"],
            "resolvable-firewall".to_string(),
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "resolvable-firewall", "FirewallArn": firewall_arn }),
            &["Firewall", "FirewallArn"],
            firewall_arn.clone(),
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyArn": policy_arn }),
            &["FirewallPolicyResponse", "FirewallPolicyArn"],
            policy_arn.clone(),
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyName": "resolvable-policy" }),
            &["FirewallPolicyResponse", "FirewallPolicyName"],
            "resolvable-policy".to_string(),
        ),
        (
            // The ARN carries the type, so `Type` is not needed beside it.
            "DescribeRuleGroup",
            json!({ "RuleGroupArn": stateful_arn }),
            &["RuleGroupResponse", "RuleGroupArn"],
            stateful_arn.clone(),
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "resolvable-stateful", "Type": "STATEFUL" }),
            &["RuleGroupResponse", "RuleGroupName"],
            "resolvable-stateful".to_string(),
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "resolvable-stateful", "RuleGroupArn": stateful_arn }),
            &["RuleGroupResponse", "RuleGroupArn"],
            stateful_arn.clone(),
        ),
    ];

    for (target, request, path, expected) in &resolves {
        let body = harness.ok(target, request).await;
        let read = string_at(&body, path);
        assert_eq!(
            read, expected,
            "case {target} {request:?}: expected {path:?} to report {expected:?}, got {body:?}"
        );
    }

    // A well-formed reference of the right type that names nothing stored is
    // still a miss, not a bad request.
    let misses: Vec<(&str, Value)> = vec![
        (
            "DescribeFirewall",
            json!({ "FirewallArn": arn_for("firewall", "never-created") }),
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "never-created" }),
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyArn": arn_for("firewall-policy", "never-created") }),
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupArn": arn_for("stateful-rulegroup", "never-created") }),
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupArn": arn_for("stateless-rulegroup", "never-created") }),
        ),
        (
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "never-created", "Type": "STATELESS" }),
        ),
        (
            // `<type>:<name>` is as ARN-legal as `<type>/<name>`, so this is a
            // well-formed reference to a firewall that was never created — a
            // miss, not a malformed ARN.
            "DescribeFirewall",
            json!({
                "FirewallArn":
                    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall:never-created"),
            }),
        ),
        (
            // The same resource type and name in another region: nothing stored
            // carries that ARN.
            "DescribeFirewall",
            json!({
                "FirewallArn":
                    format!("arn:aws:network-firewall:us-west-2:{ACCOUNT}:firewall/resolvable-firewall"),
            }),
        ),
    ];

    for (target, request) in &misses {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {target} {request:?}: expected 400 ResourceNotFoundException, \
             got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {target} {request:?}: expected the constant not-found message \
             {NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
    }
}
