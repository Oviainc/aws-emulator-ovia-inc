// SPDX-License-Identifier: CC0-1.0

//! Unit cover for the renderings `crate::state` computes in-process.

use super::*;

/// A bound and a submitted value reach the recorded decimal rendering, and a value
/// that already carries a fraction is not given a second one.
#[test]
fn a_numeric_bound_is_stated_in_decimal_form() {
    let cases = [
        ("an integral bound", "20160", "20160.0"),
        ("one", "1", "1.0"),
        ("zero", "0", "0.0"),
        ("a negative value", "-3", "-3.0"),
        ("an already-fractional value", "2.5", "2.5"),
    ];
    for (case, value, expected) in cases {
        let actual = as_decimal(value);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// Every violation class states the operation, the member path and the requirement.
#[test]
fn a_violation_clause_names_the_operation_and_the_member() {
    let cases = [
        (
            "a range maximum",
            Violation {
                path: "duration".to_string(),
                kind: ViolationKind::MaxValue("20160".to_string()),
                value: Some("20161".to_string()),
                sensitive: false,
            },
            "CreateParticipantToken.duration is required to have a maximum value of \
                 20160.0, found 20161.0",
        ),
        (
            "a range minimum",
            Violation {
                path: "duration".to_string(),
                kind: ViolationKind::MinValue("1".to_string()),
                value: Some("0".to_string()),
                sensitive: false,
            },
            "CreateParticipantToken.duration is required to have a minimum value of 1.0, \
                 found 0.0",
        ),
        (
            "a pattern",
            Violation {
                path: "stageArn".to_string(),
                kind: ViolationKind::Pattern("^arn:aws:ivs:".to_string()),
                value: Some("nope".to_string()),
                sensitive: false,
            },
            "CreateParticipantToken.stageArn does not match the required pattern \
                 ^arn:aws:ivs:",
        ),
        (
            "a minimum length",
            Violation {
                path: "stageArn".to_string(),
                kind: ViolationKind::MinLength(1),
                value: Some(String::new()),
                sensitive: false,
            },
            "CreateParticipantToken.stageArn is required to have a minimum length of 1, \
                 found 0",
        ),
        (
            "a required member",
            missing_member_violation("stageArn".to_string()),
            "CreateParticipantToken.stageArn is required",
        ),
    ];
    for (case, violation, expected) in cases {
        let actual = violation_clause("CreateParticipantToken", &violation);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The reserved namespace is the NAMESPACE, not one spelling of it, and a key that
/// merely starts with the same letters is not in it. A key whose bytes straddle the
/// four the comparison reads is not a special case: it simply does not claim the
/// namespace.
#[test]
fn only_a_key_claiming_the_reserved_namespace_is_recognised() {
    let cases = [
        ("the reserved prefix in lower case", "aws:owner", true),
        (
            "a reserved key with a qualifier, as AWS's own tags carry",
            "aws:cloudformation:stack-name",
            true,
        ),
        ("the reserved prefix in upper case", "AWS:owner", true),
        ("the reserved prefix in mixed case", "Aws:owner", true),
        ("the prefix and nothing after it", "aws:", true),
        ("the same letters without the colon", "awsome", false),
        ("the letters as a whole key", "aws", false),
        ("the prefix in the middle of a key", "team-aws:owner", false),
        ("an ordinary key", "team", false),
        ("a key whose first bytes are multi-byte", "ä-aws:owner", false),
        ("a reserved key with a multi-byte tail", "aws:ö", true),
    ];
    for (case, key, expected) in cases {
        let actual = claims_reserved_tag_namespace(key);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A refusal names the operation, the member the keys arrived in and every offending
/// key, in sorted order; a request naming none is accepted.
#[test]
fn a_reserved_key_refusal_names_the_operation_the_member_and_every_offending_key() {
    let accepted = check_tag_keys_unreserved("TagResource", "tags", ["team", "env"]);
    assert!(
        accepted.is_ok(),
        "case \"no reserved key\": expected acceptance, got {accepted:?}"
    );

    let cases: [(&str, &str, &str, Vec<&str>, &str); 3] = [
        (
            "one reserved key on TagResource",
            "TagResource",
            "tags",
            vec!["aws:owner"],
            "TagResource.tags is required to name no key in the reserved aws: namespace, \
                 found aws:owner",
        ),
        (
            "two reserved keys beside an ordinary one",
            "CreateStage",
            "tags",
            vec!["AWS:b", "team", "aws:a"],
            "CreateStage.tags is required to name no key in the reserved aws: namespace, \
                 found AWS:b, aws:a",
        ),
        (
            "a reserved key on UntagResource",
            "UntagResource",
            "tagKeys",
            vec!["aws:owner", "aws:owner"],
            "UntagResource.tagKeys is required to name no key in the reserved aws: \
                 namespace, found aws:owner",
        ),
    ];
    for (case, operation, member, keys, expected) in cases {
        let actual = check_tag_keys_unreserved(operation, member, keys);
        assert_eq!(
            actual.as_ref().err().map(String::as_str),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}
