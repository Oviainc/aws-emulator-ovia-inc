// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the exception BODIES the Resolver rule slice answers
//! with: the message of a missing rule, of a delete refused because a VPC still
//! uses the rule, and of a rule/VPC pair no association was ever made from.
//!
//! The rule these span: every Route 53 Resolver exception message opens with the
//! service's own error code in square brackets, because the HTTP status (400 for
//! every modelled Resolver error) and the exception shape name together still do
//! not say which Resolver fault occurred — the code is what tells one 400 of a
//! shape from another. The rest of the message is that fault's fixed sentence,
//! with only the values the request named interpolated into it; nothing the
//! store counted and nothing the sentence does not call for is echoed back.
//!
//! The suites that assert the same shape of body for the endpoint and tag slices
//! are [`super::resolver_endpoints`] and [`super::resource_tags`]; this one is
//! the rule slice's.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`], and the rule/endpoint/association builders are the
//! ones in [`super::resolver_rule_deletion`].

use serde_json::{json, Value};

use super::harness::{call, emulator, error_type, ok, string_member};
use super::resolver_rule_deletion::{associate, forward_rule, outbound_endpoint};

/// The Route 53 Resolver service error code for a rule id the account holds no
/// Resolver rule under, from the recorded `ResourceNotFoundException` bodies of
/// the operations that resolve a `ResolverRuleId`.
const CODE_RULE_NOT_FOUND: &str = "RSLVR-00703";

/// The code for a delete of a rule a VPC is still associated with, from the
/// recorded `ResourceInUseException` body of `DeleteResolverRule`.
const CODE_RULE_IN_USE: &str = "RSLVR-00702";

/// The code for a rule/VPC pair the service holds no association for, from the
/// recorded `ResourceNotFoundException` body of `DisassociateResolverRule`.
const CODE_ASSOCIATION_NOT_FOUND: &str = "RSLVR-00800";

/// Ids spanning what the not-found rule changes over: the form this service
/// mints, a form it never mints, and both published bounds of the modelled
/// `ResourceId` (`smithy_query(query="operation",operation="GetResolverRule")`
/// gives `ResolverRuleId -> ResourceId`, length 1..64) plus one character past
/// the upper bound, which the boundary refuses before a handler sees it.
fn unknown_rule_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "an id of the form this service mints",
            "rslvr-rr-0000000000000000".to_string(),
        ),
        ("an id of no Resolver form", "not-a-resolver-rule-id".to_string()),
        ("the shortest ResourceId (length 1)", "r".to_string()),
        ("the longest ResourceId (length 64)", "r".repeat(64)),
        (
            "an id carrying the quote character the template uses",
            "rslvr-rr-\"quoted\"".to_string(),
        ),
        (
            "an id differing from a minted one only by case",
            "RSLVR-RR-0123456789ABCDEF".to_string(),
        ),
    ]
}

/// Assert one refusal: HTTP 400, the modelled shape, the exact message and the
/// modelled resource type.
fn assert_refusal(
    case: &str,
    operation: &str,
    status: u16,
    parsed: &Value,
    expected_shape: &str,
    expected_message: &str,
    expected_resource_type: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}/{operation}: expected HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(parsed).contains(expected_shape),
        "case {case:?}/{operation}: expected {expected_shape}, got {parsed}"
    );
    let message = string_member(parsed, "Message");
    assert_eq!(
        message, expected_message,
        "case {case:?}/{operation}: expected Message {expected_message:?}, got {message:?}"
    );
    assert_eq!(
        string_member(parsed, "ResourceType"),
        expected_resource_type,
        "case {case:?}/{operation}: expected ResourceType {expected_resource_type:?}, got {parsed}"
    );
}

/// Every operation that resolves a `ResolverRuleId` answers an id it holds no
/// rule under with the one RSLVR-coded sentence, the requested id double-quoted
/// inside it, and the rule's modelled resource type.
#[tokio::test]
async fn a_missing_rule_is_refused_with_the_coded_sentence() {
    let router = emulator();
    // A stored rule, so the refusals below are about the requested id and not
    // about an empty store.
    let endpoint = outbound_endpoint(&router, "endpoint-missing-rule").await;
    let stored = forward_rule(
        &router,
        "rule-missing-rule",
        "missing.example.com",
        &endpoint,
    )
    .await;

    for (case, id) in unknown_rule_ids() {
        let requests = [
            ("GetResolverRule", json!({ "ResolverRuleId": id })),
            ("DeleteResolverRule", json!({ "ResolverRuleId": id })),
            (
                "AssociateResolverRule",
                json!({ "ResolverRuleId": id, "VPCId": "vpc-00000000000000031" }),
            ),
        ];
        for (operation, body) in requests {
            let (status, parsed) = call(&router, operation, body).await;
            let expected =
                format!(r#"[{CODE_RULE_NOT_FOUND}] Resolver rule with ID "{id}" does not exist."#);
            assert_refusal(
                case,
                operation,
                status,
                &parsed,
                "ResourceNotFoundException",
                &expected,
                "ResolverRule",
            );
            // The parts the rule is made of, asserted on their own so a future
            // reword cannot drop one of them unnoticed.
            let message = string_member(&parsed, "Message");
            assert!(
                message.starts_with(&format!("[{CODE_RULE_NOT_FOUND}] ")),
                "case {case:?}/{operation}: expected the bracketed code prefix, got {message:?}"
            );
            assert!(
                message.contains(&format!("\"{id}\"")),
                "case {case:?}/{operation}: expected the requested id double-quoted, got \
                 {message:?}"
            );
            assert!(
                message.ends_with('.'),
                "case {case:?}/{operation}: expected a terminating period, got {message:?}"
            );
        }
    }

    // An id one character past the modelled maximum never reaches the handler:
    // the boundary refuses it, so it carries no rule body at all.
    let too_long = "r".repeat(65);
    let (status, parsed) = call(
        &router,
        "GetResolverRule",
        json!({ "ResolverRuleId": too_long }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the longest ResourceId\": expected HTTP 400, got {status}: {parsed}"
    );
    assert!(
        !string_member(&parsed, "Message").contains(CODE_RULE_NOT_FOUND),
        "case \"one past the longest ResourceId\": a request the boundary refuses must not carry \
         the rule-not-found body, got {parsed}"
    );

    // And the rule the store does hold still reads back, so none of the
    // refusals touched it.
    assert_eq!(
        string_member(
            ok(
                &router,
                "GetResolverRule",
                json!({ "ResolverRuleId": stored })
            )
            .await
            .get("ResolverRule")
            .unwrap_or(&Value::Null),
            "Id"
        ),
        stored,
        "the refusals must leave the stored rule readable"
    );
}

/// One row of the in-use table: how many VPCs are associated with the rule when
/// the delete is attempted.
const IN_USE_VPCS: &[(&str, &[&str])] = &[
    ("one VPC uses the rule", &["vpc-00000000000000041"]),
    (
        "two VPCs use the rule",
        &["vpc-00000000000000042", "vpc-00000000000000043"],
    ),
    (
        "four VPCs use the rule",
        &[
            "vpc-00000000000000044",
            "vpc-00000000000000045",
            "vpc-00000000000000046",
            "vpc-00000000000000047",
        ],
    ),
];

/// A delete refused because a VPC still uses the rule answers the ONE coded
/// sentence, whatever the rule is and however many VPCs use it: the sentence is
/// entirely fixed, so neither the rule, nor any VPC, nor the number of
/// associations appears in it.
///
/// The rows differ along the count the message must NOT vary with.
#[tokio::test]
async fn an_in_use_rule_is_refused_with_the_one_fixed_sentence() {
    let expected =
        format!("[{CODE_RULE_IN_USE}] Please disassociate this resolver rule from VPC first before deleting.");

    for (index, (case, vpcs)) in IN_USE_VPCS.iter().enumerate() {
        let router = emulator();
        let endpoint = outbound_endpoint(&router, &format!("endpoint-in-use-{index}")).await;
        let rule_id = forward_rule(
            &router,
            &format!("rule-in-use-{index}"),
            &format!("in-use-{index}.example.com"),
            &endpoint,
        )
        .await;
        for vpc in *vpcs {
            associate(&router, &rule_id, vpc).await;
        }

        let (status, parsed) = call(
            &router,
            "DeleteResolverRule",
            json!({ "ResolverRuleId": rule_id }),
        )
        .await;
        assert_refusal(
            case,
            "DeleteResolverRule",
            status,
            &parsed,
            "ResourceInUseException",
            &expected,
            "ResolverRule",
        );

        // None of the values this refusal was computed from is echoed back. The
        // count is checked on the SENTENCE, since the bracketed code the
        // message opens with is digits of its own.
        let message = string_member(&parsed, "Message");
        let sentence = message
            .split_once("] ")
            .map(|(_, sentence)| sentence.to_string())
            .unwrap_or_else(|| message.clone());
        assert!(
            !message.contains(&rule_id),
            "case {case:?}: the fixed sentence must not name the rule, got {message:?}"
        );
        assert!(
            !sentence.contains(char::is_numeric),
            "case {case:?}: the fixed sentence must state no count, got {sentence:?}"
        );
        for vpc in *vpcs {
            assert!(
                !message.contains(vpc),
                "case {case:?}: the fixed sentence must not name a VPC, got {message:?}"
            );
        }
    }
}

/// One row of the association table: the pair a disassociate names, and why the
/// store holds no association for it. `{rule}` stands for the stored rule's id.
struct PairCase {
    case: &'static str,
    rule_id: &'static str,
    vpc_id: &'static str,
}

const PAIR_CASES: &[PairCase] = &[
    PairCase {
        case: "a rule the store holds nothing under, and any VPC",
        rule_id: "rslvr-rr-0000000000000000",
        vpc_id: "vpc-00000000000000051",
    },
    PairCase {
        case: "the stored rule, and a VPC it was never associated with",
        rule_id: "{rule}",
        vpc_id: "vpc-00000000000000052",
    },
    PairCase {
        case: "ids of no AWS form on both halves of the pair",
        rule_id: "not-a-rule-id",
        vpc_id: "not-a-vpc-id",
    },
    PairCase {
        case: "the shortest ResourceId on both halves (length 1)",
        rule_id: "r",
        vpc_id: "v",
    },
    PairCase {
        case: "ids carrying the quote character the template uses",
        rule_id: "rslvr-rr-\"quoted\"",
        vpc_id: "vpc-\"quoted\"",
    },
];

/// A rule/VPC pair the store holds no association for is refused with the coded
/// sentence that names the missing ASSOCIATION — not a property of the rule —
/// with the rule and the VPC the request named as its only variable parts.
#[tokio::test]
async fn a_pair_with_no_association_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-pair-bodies").await;
    let rule_id = forward_rule(&router, "rule-pair-bodies", "pairs.example.com", &endpoint).await;
    let held_vpc = "vpc-00000000000000050";
    associate(&router, &rule_id, held_vpc).await;

    let mut rows: Vec<(&str, String, String)> = PAIR_CASES
        .iter()
        .map(|row| {
            (
                row.case,
                row.rule_id.replace("{rule}", &rule_id),
                row.vpc_id.to_string(),
            )
        })
        .collect();
    // The longest modelled `ResourceId` (length 64) on both halves.
    rows.push((
        "the longest ResourceId on both halves (length 64)",
        "r".repeat(64),
        "v".repeat(64),
    ));

    for (case, rule, vpc) in &rows {
        let (status, parsed) = call(
            &router,
            "DisassociateResolverRule",
            json!({ "ResolverRuleId": rule, "VPCId": vpc }),
        )
        .await;
        let expected = format!(
            r#"[{CODE_ASSOCIATION_NOT_FOUND}] Resolver Rule Association between Resolver Rule "{rule}" and VPC "{vpc}" does not exist."#
        );
        assert_refusal(
            case,
            "DisassociateResolverRule",
            status,
            &parsed,
            "ResourceNotFoundException",
            &expected,
            "ResolverRuleAssociation",
        );
    }

    // The pair the store DOES hold an association for disassociates; asking
    // again addresses an association that is gone, which is the same body.
    ok(
        &router,
        "DisassociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": held_vpc }),
    )
    .await;
    let (status, parsed) = call(
        &router,
        "DisassociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": held_vpc }),
    )
    .await;
    let expected = format!(
        r#"[{CODE_ASSOCIATION_NOT_FOUND}] Resolver Rule Association between Resolver Rule "{rule_id}" and VPC "{held_vpc}" does not exist."#
    );
    assert_refusal(
        "a pair whose association was already disassociated",
        "DisassociateResolverRule",
        status,
        &parsed,
        "ResourceNotFoundException",
        &expected,
        "ResolverRuleAssociation",
    );
}
