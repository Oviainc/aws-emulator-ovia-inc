// SPDX-License-Identifier: CC0-1.0

//! The documented bound on the names a rules file declares, end to end over HTTP.
//!
//! The rule under test: "The combined length of a rule group namespace and a rule group
//! name cannot exceed 721 UTF-8 bytes" — the `<important>` callout both
//! `CreateRuleGroupsNamespace` and `PutRuleGroupsNamespace` carry in their own
//! documentation. A request declaring a rule group past that bound is refused with the
//! modelled `ValidationException` (400) and nothing is stored or replaced; a request at
//! or under it is accepted and the rules file round-trips byte for byte.
//!
//! Every suite spans what that rule generalises over — where the bytes sit (a long
//! namespace name against a long rule group name), the published bound and one byte past
//! it, the UTF-8 width of the group name, which group of a multi-group file carries it,
//! the YAML form the groups sequence is written in (block or flow), and a blob that is
//! not a rules file at all — rather than one request's values.
//!
//! The request harness (`Amp`, `Reply`) and the create/put/describe calls are the ones
//! `workspace_lifecycle`, `rule_groups_namespace_lifecycle` and
//! `rule_groups_namespace_update` already build.

use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::rule_groups_namespace_update::{blob, put_body, put_namespace};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    NAMESPACE_STATUS_ACTIVE, NAMESPACE_STATUS_UPDATING, RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES,
};

/// The bound as both operations' documentation publishes it, written out here so the
/// table is read against the PUBLISHED number rather than against whatever the crate
/// holds.
const BOUND: usize = 721;

/// The refusal's `message`: the requirement, worded as the service documents the bound
/// itself. Carries nothing from the request — aps keeps its error text free of caller
/// input.
const EXPECTED_MESSAGE: &str = "The combined length of a rule group namespace and a rule \
                                group name cannot exceed 721 UTF-8 bytes";

/// A rules file declaring one rule group, in the block form a Prometheus rules file is
/// written in (`groups:` -> item -> `name`, the structure `RuleGroupsNamespaceData`
/// documents).
fn one_group(group: &str) -> Vec<u8> {
    format!(
        "groups:\n  - name: {group}\n    interval: 1m\n    rules:\n      - record: job:cpu\n        expr: sum(rate(cpu[5m]))\n"
    )
    .into_bytes()
}

/// The same file in YAML's FLOW form (which is also its JSON spelling): the bound is on
/// the names a rules file declares, not on the form it is written in.
fn one_group_flow(group: &str) -> Vec<u8> {
    format!("{{\"groups\": [{{\"name\": \"{group}\", \"rules\": []}}]}}").into_bytes()
}

/// A rules file declaring two rule groups: the bound holds for every group a file
/// declares, not only for the first one.
fn two_groups(first: &str, second: &str) -> Vec<u8> {
    format!("groups:\n  - name: {first}\n    rules: []\n  - name: {second}\n    rules: []\n")
        .into_bytes()
}

/// A rule group name of `bytes` ASCII bytes.
fn ascii_group(bytes: usize) -> String {
    "g".repeat(bytes)
}

/// A rule group name of `characters` copies of `✓` (U+2713), which is THREE UTF-8 bytes:
/// the bound counts bytes, so this name reaches it in a third of the characters.
fn multibyte_group(characters: usize) -> String {
    "\u{2713}".repeat(characters)
}

/// A namespace name of `bytes` bytes, distinguished from every other row's by `tag`.
/// `RuleGroupsNamespaceName` is `len 1..128`, pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*`, which
/// every name here satisfies.
fn namespace_name(bytes: usize, tag: char) -> String {
    assert!(
        (2..=128).contains(&bytes),
        "a tagged namespace name needs at least two bytes"
    );
    format!("n{}{tag}", "0".repeat(bytes - 2))
}

/// One row of the table: the namespace the rules file is written under, the file, and
/// whether the service must store it or refuse it.
struct Case {
    case: &'static str,
    name: String,
    data: Vec<u8>,
    /// `true` when the request is within the bound (202), `false` when it is past it
    /// (400).
    accepted: bool,
}

/// The table spans what the rule generalises over: the combined length at one byte under
/// the published bound, exactly at it and one byte past it; the split of those bytes
/// between the namespace name (at its modelled length minimum, a middling length and its
/// modelled maximum) and the rule group name; the UTF-8 width of the group name; which
/// group of a multi-group file is the long one; the block and flow forms of the groups
/// sequence; and files that declare no rule group name at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "combined length one byte under the bound",
            name: namespace_name(10, '1'),
            data: one_group(&ascii_group(BOUND - 10 - 1)),
            accepted: true,
        },
        Case {
            case: "combined length exactly at the published bound of 721",
            name: namespace_name(10, '2'),
            data: one_group(&ascii_group(BOUND - 10)),
            accepted: true,
        },
        Case {
            case: "combined length one byte past the bound",
            name: namespace_name(10, '3'),
            data: one_group(&ascii_group(BOUND - 10 + 1)),
            accepted: false,
        },
        Case {
            case: "namespace name at its modelled maximum of 128, combined at the bound",
            name: namespace_name(128, '4'),
            data: one_group(&ascii_group(BOUND - 128)),
            accepted: true,
        },
        Case {
            case: "namespace name at its modelled maximum of 128, one byte past the bound",
            name: namespace_name(128, '5'),
            data: one_group(&ascii_group(BOUND - 128 + 1)),
            accepted: false,
        },
        Case {
            case: "namespace name at its modelled minimum of 1, combined at the bound",
            name: "a".to_string(),
            data: one_group(&ascii_group(BOUND - 1)),
            accepted: true,
        },
        Case {
            case: "namespace name at its modelled minimum of 1, one byte past the bound",
            name: "b".to_string(),
            data: one_group(&ascii_group(BOUND - 1 + 1)),
            accepted: false,
        },
        Case {
            // 237 characters of three bytes each: 711 bytes, 721 combined.
            case: "a multibyte group name whose UTF-8 bytes reach the bound exactly",
            name: namespace_name(10, '6'),
            data: one_group(&multibyte_group((BOUND - 10) / 3)),
            accepted: true,
        },
        Case {
            // 238 characters: 714 bytes, 724 combined — past the bound in BYTES while
            // far under it in characters.
            case: "a multibyte group name whose UTF-8 bytes pass the bound",
            name: namespace_name(10, '7'),
            data: one_group(&multibyte_group((BOUND - 10) / 3 + 1)),
            accepted: false,
        },
        Case {
            case: "the second group of a file is measured too",
            name: namespace_name(10, '8'),
            data: two_groups("cpu", &ascii_group(BOUND - 10 + 1)),
            accepted: false,
        },
        Case {
            case: "every group of a two-group file within the bound",
            name: namespace_name(10, '9'),
            data: two_groups("cpu", &ascii_group(BOUND - 10)),
            accepted: true,
        },
        Case {
            case: "the flow form of a groups sequence, one byte past the bound",
            name: namespace_name(10, 'A'),
            data: one_group_flow(&ascii_group(BOUND - 10 + 1)),
            accepted: false,
        },
        Case {
            case: "the flow form of a groups sequence, at the bound",
            name: namespace_name(10, 'B'),
            data: one_group_flow(&ascii_group(BOUND - 10)),
            accepted: true,
        },
        Case {
            case: "a blob that is not a YAML document declares no rule group name",
            name: namespace_name(10, 'C'),
            data: (0..4096u32).map(|byte| (byte % 251) as u8).collect(),
            accepted: true,
        },
        Case {
            case: "an empty rules file declares no rule group name",
            name: namespace_name(10, 'D'),
            data: Vec::new(),
            accepted: true,
        },
        Case {
            case: "a long name that is not a rule group's is not measured",
            name: namespace_name(10, 'E'),
            data: format!("name: {}\n", ascii_group(900)).into_bytes(),
            accepted: true,
        },
        Case {
            case: "a long name inside a rule rather than a rule group is not measured",
            name: namespace_name(10, 'F'),
            data: format!(
                "groups:\n  - name: cpu\n    rules:\n      - name: {}\n        expr: up\n",
                ascii_group(900)
            )
            .into_bytes(),
            accepted: true,
        },
    ]
}

/// Every assertion a refusal makes: the modelled `ValidationException` at 400, stating
/// the published requirement, with no `fieldList` — the bound spans two members, so no
/// single member is at fault.
fn assert_refusal(case: &str, reply: &crate::workspace_lifecycle::Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a rules file past the combined-name bound must be refused 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        EXPECTED_MESSAGE,
        "case {case:?}: the refusal must state the published requirement, got {reply:?}"
    );
    assert!(
        reply.body["fieldList"].is_null(),
        "case {case:?}: the bound spans two members, so no fieldList entry names one, got {reply:?}"
    );
}

/// The emulator's bound is the published one, so every row of the table above is read
/// against the same number the documentation states.
#[test]
fn the_enforced_bound_is_the_published_one() {
    assert_eq!(
        RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES, BOUND,
        "the enforced combined-name bound must be the published {BOUND} UTF-8 bytes, \
         got {RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES}"
    );
}

#[tokio::test]
async fn create_refuses_a_rules_file_past_the_combined_name_bound() {
    let amp = Amp::new();
    let region = "us-east-2";
    let workspace_id = amp.active_workspace(region, "rule-group-name-bound").await;

    let cases = cases();
    for case in &cases {
        let reply = create_namespace(
            &amp,
            region,
            &workspace_id,
            &create_body(&case.name, &case.data, &[]),
        )
        .await;
        if case.accepted {
            assert_eq!(
                reply.status, 202,
                "case {:?}: a rules file within the bound must be accepted 202, got {reply:?}",
                case.case
            );
        } else {
            assert_refusal(case.case, &reply);
        }
    }

    // The clock owes every accepted namespace its move to ACTIVE; a refused create left
    // nothing to move.
    wait_for_due_transition();
    for case in &cases {
        let described = describe_namespace(&amp, region, &workspace_id, &case.name).await;
        if case.accepted {
            assert_eq!(
                described.status, 200,
                "case {:?}: the accepted namespace must be readable back, got {described:?}",
                case.case
            );
            let namespace = &described.body["ruleGroupsNamespace"];
            assert_eq!(
                blob(case.case, &namespace["data"]),
                case.data,
                "case {:?}: the stored rules file must be the bytes the request carried",
                case.case
            );
            assert_eq!(
                text(&namespace["status"]["statusCode"]),
                NAMESPACE_STATUS_ACTIVE,
                "case {:?}: the namespace must be {NAMESPACE_STATUS_ACTIVE} once its \
                 transition is due, got {described:?}",
                case.case
            );
        } else {
            assert_eq!(
                described.status, 404,
                "case {:?}: a refused create must store nothing, got {described:?}",
                case.case
            );
            assert_eq!(
                described.error_code(),
                "ResourceNotFoundException",
                "case {:?}: expected ResourceNotFoundException for the namespace the \
                 refused create did not make, got {described:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn put_refuses_a_rules_file_past_the_combined_name_bound() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let workspace_id = amp.active_workspace(region, "rule-group-name-bound").await;

    // Every row acts on an EXISTING namespace holding a rules file within the bound, so
    // what a refused update must leave untouched is the file below.
    let held: Vec<u8> = one_group("cpu");
    let cases = cases();
    for case in &cases {
        let created = create_namespace(
            &amp,
            region,
            &workspace_id,
            &create_body(&case.name, &held, &[]),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: the namespace to update must be created, got {created:?}",
            case.case
        );
    }
    wait_for_due_transition();

    for case in &cases {
        let reply = put_namespace(
            &amp,
            region,
            &workspace_id,
            &case.name,
            &put_body(&case.data),
        )
        .await;
        let described = describe_namespace(&amp, region, &workspace_id, &case.name).await;
        let namespace = &described.body["ruleGroupsNamespace"];
        if case.accepted {
            assert_eq!(
                reply.status, 202,
                "case {:?}: a rules file within the bound must be accepted 202, got {reply:?}",
                case.case
            );
            assert_eq!(
                blob(case.case, &namespace["data"]),
                case.data,
                "case {:?}: the accepted update must replace the stored rules file",
                case.case
            );
            assert_eq!(
                text(&namespace["status"]["statusCode"]),
                NAMESPACE_STATUS_UPDATING,
                "case {:?}: the namespace must report {NAMESPACE_STATUS_UPDATING} while \
                 the update is in flight, got {described:?}",
                case.case
            );
        } else {
            assert_refusal(case.case, &reply);
            assert_eq!(
                blob(case.case, &namespace["data"]),
                held,
                "case {:?}: a refused update must leave the stored rules file alone",
                case.case
            );
            assert_eq!(
                text(&namespace["status"]["statusCode"]),
                NAMESPACE_STATUS_ACTIVE,
                "case {:?}: a refused update must leave the namespace \
                 {NAMESPACE_STATUS_ACTIVE}, got {described:?}",
                case.case
            );
        }
    }
}
