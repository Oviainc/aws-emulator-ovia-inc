// SPDX-License-Identifier: CC0-1.0

//! `GetCustomRulePolicy`: the Guard policy body a Config Custom Policy rule was put
//! with, read back out of state.
//!
//! Two rules are pinned here, each over a table of inputs rather than one recorded
//! case:
//!
//! 1. `GetCustomRulePolicy` reports the `Source.CustomPolicyDetails.PolicyText`
//!    persisted for the named rule, byte for byte and unchanged by reading it — the
//!    one member `DescribeConfigRules` withholds;
//! 2. a request that does not name a Custom Policy rule this account holds is refused
//!    with HTTP 400 `NoSuchConfigRuleException`, the only error the operation models.
//!
//! The harness (`Emulator`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds; every case drives the emulator the way
//! a client does, through the `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// The only published `PolicyRuntime` value: `PolicyRuntime`'s pattern is
/// `^guard\-2\.x\.x$` (`smithy_query(query="shape", shape="PolicyRuntime")`).
const POLICY_RUNTIME: &str = "guard-2.x.x";

/// The published `PolicyText` length maximum, from
/// `smithy_query(query="shape", shape="PolicyText")`: length 0..10000.
const POLICY_TEXT_MAX: usize = 10_000;

/// A Guard policy body of exactly `len` characters: a padded comment line followed by
/// a rule, so the body stays plausible Guard at every size.
fn policy_of_length(len: usize) -> String {
    const TAIL: &str = "\nrule always { true }";
    assert!(
        len > TAIL.len() + 2,
        "policy_of_length: {len} is too short to pad"
    );
    let mut body = String::from("# ");
    body.push_str(&"p".repeat(len - TAIL.len() - 2));
    body.push_str(TAIL);
    assert_eq!(body.chars().count(), len, "policy_of_length built {body:?}");
    body
}

/// The `PutConfigRule` body for a Custom Policy rule named `name` carrying `policy`.
fn policy_rule(name: &str, policy: &str, debug_logging: bool) -> Value {
    json!({
        "ConfigRule": {
            "ConfigRuleName": name,
            "Source": {
                "Owner": "CUSTOM_POLICY",
                "CustomPolicyDetails": {
                    "PolicyRuntime": POLICY_RUNTIME,
                    "PolicyText": policy,
                    "EnableDebugLogDelivery": debug_logging,
                },
            },
        }
    })
}

#[tokio::test]
async fn a_custom_policy_rules_policy_body_round_trips_through_state() {
    // Rows differ along what the rule generalises over: the size of the policy body
    // (empty, one line, many lines, one short of the published maximum, exactly the
    // maximum), the characters in it (quotes, backslashes, tabs, non-ASCII, trailing
    // newline) and whether debug logging is on.
    let long = policy_of_length(POLICY_TEXT_MAX - 1);
    let at_max = policy_of_length(POLICY_TEXT_MAX);
    let cases: [(&str, &str, bool); 8] = [
        ("an empty policy body", "", false),
        ("a one-line policy", "rule always { true }", false),
        (
            "a multi-line policy with a trailing newline",
            "rule tagged when resourceType == 'AWS::DynamoDB::Table' {\n  \
             configuration.tags != null\n}\n",
            true,
        ),
        (
            "a policy carrying quotes, backslashes and tabs",
            "rule quoted {\n\tconfiguration.name == \"a\\\"b\"\n}",
            false,
        ),
        (
            "a policy carrying non-ASCII characters",
            "# règle ✓ 日本語\nrule always { true }",
            true,
        ),
        (
            "a policy that is only whitespace",
            "   \n\t\n",
            false,
        ),
        ("one character short of the maximum", long.as_str(), false),
        ("exactly the maximum length", at_max.as_str(), true),
    ];

    for (case, policy, debug_logging) in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        emulator
            .ok("PutConfigRule", &policy_rule("guard-rule", policy, debug_logging))
            .await;

        // Captured BEFORE any read of the policy, so a mutation performed by the very
        // first `GetCustomRulePolicy` would show up below.
        let described_before = emulator.ok("DescribeConfigRules", &json!({})).await;

        let got = emulator
            .ok(
                "GetCustomRulePolicy",
                &json!({ "ConfigRuleName": "guard-rule" }),
            )
            .await;
        assert_eq!(
            got["PolicyText"],
            json!(policy),
            "case {case:?}: expected the {} character policy body that was put, got {got}",
            policy.chars().count()
        );

        // Reading the policy must not change what a later caller sees: not the policy
        // itself, and not the rule the describe path reports.
        let again = emulator
            .ok(
                "GetCustomRulePolicy",
                &json!({ "ConfigRuleName": "guard-rule" }),
            )
            .await;
        assert_eq!(
            again, got,
            "case {case:?}: a second GetCustomRulePolicy must report the same body, got {again}"
        );
        let described_after = emulator.ok("DescribeConfigRules", &json!({})).await;
        assert_eq!(
            described_after, described_before,
            "case {case:?}: reading the policy must not change the described rule, got \
             {described_after}"
        );

        // The describe path still withholds the body, which is why this operation
        // exists: the two reads disagree only on that member.
        assert_eq!(
            described_after["ConfigRules"][0]["Source"]["CustomPolicyDetails"]["PolicyText"],
            json!(""),
            "case {case:?}: DescribeConfigRules must keep withholding the policy body, \
             got {described_after}"
        );

        // An update to the rule replaces the body the next read reports. The new body
        // is the old one behind a comment, clipped to the published maximum so even the
        // longest row stays inside it.
        let updated: String = "# updated\n"
            .chars()
            .chain(policy.chars())
            .take(POLICY_TEXT_MAX)
            .collect();
        assert_ne!(
            updated, policy,
            "case {case:?}: the update must carry a different body"
        );
        emulator
            .ok(
                "PutConfigRule",
                &policy_rule("guard-rule", &updated, !debug_logging),
            )
            .await;
        let after_update = emulator
            .ok(
                "GetCustomRulePolicy",
                &json!({ "ConfigRuleName": "guard-rule" }),
            )
            .await;
        assert_eq!(
            after_update["PolicyText"],
            json!(updated),
            "case {case:?}: expected the updated policy body, got {after_update}"
        );
    }
}

#[tokio::test]
async fn each_rule_keeps_its_own_policy_body() {
    // Several Custom Policy rules in one account: each read reports that rule's body,
    // so the operation is reading state rather than echoing the last write.
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    let rules: [(&str, &str); 3] = [
        ("alpha", "rule alpha { true }"),
        ("beta", "rule beta {\n  configuration.tags != null\n}"),
        ("gamma", ""),
    ];
    for (name, policy) in rules {
        emulator
            .ok("PutConfigRule", &policy_rule(name, policy, false))
            .await;
    }

    for (name, policy) in rules {
        let got = emulator
            .ok("GetCustomRulePolicy", &json!({ "ConfigRuleName": name }))
            .await;
        assert_eq!(
            got["PolicyText"],
            json!(policy),
            "case {name:?}: expected that rule's own policy body, got {got}"
        );
    }
}

#[tokio::test]
async fn a_request_that_names_no_custom_policy_rule_is_refused() {
    // Rows differ along HOW the request fails to name a Custom Policy rule in state:
    // no name at all, a name nothing holds, the name of a rule of each other source
    // owner, and the name of a Custom Policy rule that has since been deleted.
    struct Case {
        case: &'static str,
        prelude: Vec<(&'static str, Value)>,
        body: Value,
    }

    let cases = [
        Case {
            case: "no rule in the account at all",
            prelude: vec![],
            body: json!({ "ConfigRuleName": "absent" }),
        },
        Case {
            case: "a name no rule in the account carries",
            prelude: vec![(
                "PutConfigRule",
                policy_rule("guard-rule", "rule always { true }", false),
            )],
            body: json!({ "ConfigRuleName": "other-rule" }),
        },
        Case {
            case: "no ConfigRuleName in the request",
            prelude: vec![(
                "PutConfigRule",
                policy_rule("guard-rule", "rule always { true }", false),
            )],
            body: json!({}),
        },
        Case {
            case: "a Config managed rule",
            prelude: vec![("PutConfigRule", managed_rule("managed"))],
            body: json!({ "ConfigRuleName": "managed" }),
        },
        Case {
            case: "a Custom Lambda rule",
            prelude: vec![(
                "PutConfigRule",
                json!({
                    "ConfigRule": {
                        "ConfigRuleName": "lambda",
                        "Source": {
                            "Owner": "CUSTOM_LAMBDA",
                            "SourceIdentifier":
                                "arn:aws:lambda:us-east-1:000000000000:function:evaluate",
                        },
                    }
                }),
            )],
            body: json!({ "ConfigRuleName": "lambda" }),
        },
        Case {
            case: "a Custom Policy rule that was deleted",
            prelude: vec![
                (
                    "PutConfigRule",
                    policy_rule("guard-rule", "rule always { true }", false),
                ),
                (
                    "DeleteConfigRule",
                    json!({ "ConfigRuleName": "guard-rule" }),
                ),
            ],
            body: json!({ "ConfigRuleName": "guard-rule" }),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        for (operation, body) in &case.prelude {
            emulator.ok(operation, body).await;
        }

        let (status, code) = emulator.refused("GetCustomRulePolicy", &case.body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigRuleException"),
            "case {:?}: expected HTTP 400 NoSuchConfigRuleException, got {status} {code}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_policy_body_past_the_published_maximum_is_refused() {
    // One past the published `PolicyText` length maximum: the rule is never created, so
    // nothing can be read back for it.
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    let too_long = policy_of_length(POLICY_TEXT_MAX + 1);

    let (status, code) = emulator
        .refused("PutConfigRule", &policy_rule("too-long", &too_long, false))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"one past the maximum\": expected HTTP 400 ValidationException for a {} \
         character policy body, got {status} {code}",
        too_long.chars().count()
    );

    let (status, code) = emulator
        .refused(
            "GetCustomRulePolicy",
            &json!({ "ConfigRuleName": "too-long" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoSuchConfigRuleException"),
        "case \"one past the maximum\": the refused rule must not exist, got {status} {code}"
    );
}
