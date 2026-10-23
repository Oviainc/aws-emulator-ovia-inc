// SPDX-License-Identifier: CC0-1.0

//! What `DescribeConfigRuleEvaluationStatus` reports about a Config rule.
//!
//! Three rules are pinned here, each over a table of inputs rather than one recorded
//! case:
//!
//! 1. the status of a rule is computed from the rule in state: it carries the identity
//!    (`ConfigRuleName`, `ConfigRuleArn`, `ConfigRuleId`) and the activation instant
//!    the write persisted, and a rule whose status Config does not publish (a Custom
//!    Lambda rule) is not reported at all;
//! 2. writing a rule is what makes Config evaluate it, and that evaluation pass
//!    completes on its own a span later — until then the rule reports
//!    `FirstEvaluationStarted` false and no successful invocation, afterwards it
//!    reports both, and the report is stable across reads;
//! 3. the operation selects by `ConfigRuleNames`, refuses a name no rule carries, pages
//!    at the `Limit` the request asked for, and refuses a `NextToken` it never issued.
//!
//! The harness (`Emulator`, `managed_rule`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds; every case drives the emulator the way
//! a client does, through the `MultiServiceRouter`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// Longer than the span the clock owes an evaluation pass (500 ms), so a read after it
/// must see the completed pass.
const PAST_THE_PASS: Duration = Duration::from_millis(700);

/// The page size `DescribeConfigRuleEvaluationStatus` serves a request that states no
/// `Limit` (`RuleLimit`'s modelled default is 0: "no stated limit").
const DEFAULT_PAGE_SIZE: usize = 25;

/// Every published `Owner` value, from
/// `smithy_query(query="shape", shape="Owner")` on this service's model: `CUSTOM_LAMBDA`,
/// `AWS`, `CUSTOM_POLICY`.
const OWNER_AWS: &str = "AWS";
/// See [`OWNER_AWS`].
const OWNER_CUSTOM_LAMBDA: &str = "CUSTOM_LAMBDA";
/// See [`OWNER_AWS`].
const OWNER_CUSTOM_POLICY: &str = "CUSTOM_POLICY";

/// A rule named `name` whose source has `owner`.
fn rule_owned_by(name: &str, owner: &str) -> Value {
    let source = match owner {
        OWNER_CUSTOM_POLICY => json!({
            "Owner": OWNER_CUSTOM_POLICY,
            "CustomPolicyDetails": {
                "PolicyRuntime": "guard-2.x.x",
                "PolicyText": "rule always { true }",
            },
        }),
        OWNER_CUSTOM_LAMBDA => json!({
            "Owner": OWNER_CUSTOM_LAMBDA,
            "SourceIdentifier": "arn:aws:lambda:us-east-1:000000000000:function:evaluate",
        }),
        _ => json!({ "Owner": OWNER_AWS, "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" }),
    };
    json!({ "ConfigRule": { "ConfigRuleName": name, "Source": source } })
}

/// The reported statuses of `body`, as a list.
fn statuses(case: &str, body: &Value) -> Vec<Value> {
    body["ConfigRulesEvaluationStatus"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ConfigRulesEvaluationStatus list, got {body}")
        })
        .clone()
}

/// The names the statuses of `body` carry, in the order they were reported.
fn reported_names(case: &str, body: &Value) -> Vec<String> {
    statuses(case, body)
        .iter()
        .map(|status| {
            status["ConfigRuleName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigRuleName in {status}"))
                .to_string()
        })
        .collect()
}

/// `member` of `status` as an epoch-seconds instant, or `None` when it is absent.
fn instant(status: &Value, member: &str) -> Option<f64> {
    status.get(member).and_then(Value::as_f64)
}

#[tokio::test]
async fn a_status_carries_the_identity_and_activation_time_the_write_persisted() {
    // Rows differ along the rule's source owner — the one class Config publishes no
    // status for is the Custom Lambda rule — and along how many rules the account
    // holds.
    struct Case {
        name: &'static str,
        rules: &'static [(&'static str, &'static str)],
        reported: &'static [&'static str],
    }

    let cases = [
        Case {
            name: "no rules at all",
            rules: &[],
            reported: &[],
        },
        Case {
            name: "one managed rule",
            rules: &[("managed", OWNER_AWS)],
            reported: &["managed"],
        },
        Case {
            name: "a Custom Policy rule beside a managed one",
            rules: &[("managed", OWNER_AWS), ("policy", OWNER_CUSTOM_POLICY)],
            reported: &["managed", "policy"],
        },
        Case {
            name: "a Custom Lambda rule carries no status",
            rules: &[("lambda", OWNER_CUSTOM_LAMBDA)],
            reported: &[],
        },
        Case {
            name: "one rule of every owner",
            rules: &[
                ("managed", OWNER_AWS),
                ("lambda", OWNER_CUSTOM_LAMBDA),
                ("policy", OWNER_CUSTOM_POLICY),
            ],
            reported: &["managed", "policy"],
        },
    ];

    for case in cases {
        let label = case.name;
        let emulator = Emulator::in_region("eu-west-1");
        if !case.rules.is_empty() {
            with_recorder_and_channel(&emulator).await;
        }
        for (name, owner) in case.rules {
            emulator
                .ok("PutConfigRule", &rule_owned_by(name, owner))
                .await;
        }

        let body = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
            .await;
        let names = reported_names(label, &body);
        assert_eq!(
            names, case.reported,
            "case {label:?}: expected statuses for {:?}, got {names:?}",
            case.reported
        );

        // Every reported status must agree with the rule DescribeConfigRules reports:
        // same ARN, same id, both minted by the write.
        let described = emulator.ok("DescribeConfigRules", &json!({})).await;
        for status in statuses(label, &body) {
            let name = status["ConfigRuleName"].as_str().unwrap_or_default();
            let rule = described["ConfigRules"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .find(|rule| rule["ConfigRuleName"].as_str() == Some(name))
                .cloned()
                .unwrap_or_else(|| panic!("case {label:?}: {name} must be a rule, got {described}"));
            for member in ["ConfigRuleArn", "ConfigRuleId"] {
                assert_eq!(
                    status[member], rule[member],
                    "case {label:?}: expected {member} {} for {name}, got {}",
                    rule[member], status[member]
                );
            }
            let activated = instant(&status, "FirstActivatedTime");
            assert!(
                activated.is_some_and(|seconds| seconds > 0.0),
                "case {label:?}: expected a FirstActivatedTime for {name}, got {status}"
            );
            assert!(
                instant(&status, "LastDeactivatedTime").is_none(),
                "case {label:?}: a rule nothing turned off reports no LastDeactivatedTime, \
                 got {status}"
            );
            for member in [
                "LastFailedInvocationTime",
                "LastFailedEvaluationTime",
                "LastErrorCode",
                "LastErrorMessage",
            ] {
                assert!(
                    status.get(member).is_none_or(Value::is_null),
                    "case {label:?}: nothing failed for {name}, so {member} must be absent, \
                     got {status}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_evaluation_pass_a_write_owes_completes_on_its_own() {
    // Rows differ along the rule the write creates: a managed rule, a periodic managed
    // rule, and a Custom Policy rule.
    let cases: [(&str, Value); 3] = [
        ("a managed rule", rule_owned_by("managed", OWNER_AWS)),
        (
            "a periodic managed rule",
            json!({
                "ConfigRule": {
                    "ConfigRuleName": "periodic",
                    "Source": {
                        "Owner": OWNER_AWS,
                        "SourceIdentifier": "IAM_PASSWORD_POLICY",
                    },
                    "MaximumExecutionFrequency": "TwentyFour_Hours",
                }
            }),
        ),
        (
            "a Custom Policy rule",
            rule_owned_by("policy", OWNER_CUSTOM_POLICY),
        ),
    ];

    for (case, rule) in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        emulator.ok("PutConfigRule", &rule).await;

        // Straight after the write the pass Config owes the rule has not completed.
        let body = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
            .await;
        let pending = statuses(case, &body)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected one status, got {body}"));
        assert!(
            !pending["FirstEvaluationStarted"]
                .as_bool()
                .unwrap_or(false),
            "case {case:?}: the first pass has not completed yet, so FirstEvaluationStarted \
             must be false, got {pending}"
        );
        for member in ["LastSuccessfulInvocationTime", "LastSuccessfulEvaluationTime"] {
            assert!(
                instant(&pending, member).is_none(),
                "case {case:?}: no pass has completed yet, so {member} must be absent, \
                 got {pending}"
            );
        }

        // The completion is owed by the clock, not by the read.
        tokio::time::sleep(PAST_THE_PASS).await;
        let body = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
            .await;
        let settled = statuses(case, &body)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected one status, got {body}"));
        assert!(
            settled["FirstEvaluationStarted"].as_bool().unwrap_or(false),
            "case {case:?}: the pass has completed, so FirstEvaluationStarted must be true, \
             got {settled}"
        );
        let activated = instant(&settled, "FirstActivatedTime")
            .unwrap_or_else(|| panic!("case {case:?}: expected a FirstActivatedTime, {settled}"));
        let invoked = instant(&settled, "LastSuccessfulInvocationTime").unwrap_or_else(|| {
            panic!("case {case:?}: expected a LastSuccessfulInvocationTime, got {settled}")
        });
        let evaluated = instant(&settled, "LastSuccessfulEvaluationTime").unwrap_or_else(|| {
            panic!("case {case:?}: expected a LastSuccessfulEvaluationTime, got {settled}")
        });
        assert!(
            activated <= invoked && invoked <= evaluated,
            "case {case:?}: expected FirstActivatedTime <= LastSuccessfulInvocationTime <= \
             LastSuccessfulEvaluationTime, got {activated} <= {invoked} <= {evaluated}"
        );

        // The read reports state; a second read must report the same instants.
        let again = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must report the same status, got {again}"
        );

        // An update re-evaluates the rule: the rule has still evaluated at least once,
        // and once the new pass completes its instants have moved forward.
        emulator.ok("PutConfigRule", &rule).await;
        let updating = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
            .await;
        let updating = statuses(case, &updating)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected one status after the update"));
        assert!(
            updating["FirstEvaluationStarted"].as_bool().unwrap_or(false),
            "case {case:?}: a rule that has evaluated once keeps FirstEvaluationStarted \
             true while the next pass runs, got {updating}"
        );
        assert_eq!(
            instant(&updating, "LastSuccessfulEvaluationTime"),
            Some(evaluated),
            "case {case:?}: the pass in flight must not move LastSuccessfulEvaluationTime \
             yet, got {updating}"
        );

        tokio::time::sleep(PAST_THE_PASS).await;
        let updated = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
            .await;
        let updated = statuses(case, &updated)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected one status after the pass"));
        let re_evaluated = instant(&updated, "LastSuccessfulEvaluationTime").unwrap_or_else(|| {
            panic!("case {case:?}: expected a LastSuccessfulEvaluationTime, got {updated}")
        });
        assert!(
            re_evaluated > evaluated,
            "case {case:?}: the update's pass must move LastSuccessfulEvaluationTime past \
             {evaluated}, got {re_evaluated}"
        );
        assert_eq!(
            instant(&updated, "FirstActivatedTime"),
            Some(activated),
            "case {case:?}: an update keeps the activation instant, got {updated}"
        );
    }
}

#[tokio::test]
async fn requested_names_select_statuses_and_an_unknown_name_is_refused() {
    // Rows differ along which names the request carries: none, one, several, a name no
    // rule carries, and a known name beside an unknown one.
    let cases: [(&str, Value, Option<&[&str]>); 6] = [
        ("no names at all", json!({}), Some(&["a-rule", "b-rule", "c-rule"])),
        (
            "one name",
            json!({ "ConfigRuleNames": ["b-rule"] }),
            Some(&["b-rule"]),
        ),
        (
            "two names, requested out of order",
            json!({ "ConfigRuleNames": ["c-rule", "a-rule"] }),
            Some(&["a-rule", "c-rule"]),
        ),
        (
            "a name that names a Custom Lambda rule",
            json!({ "ConfigRuleNames": ["lambda-rule"] }),
            Some(&[]),
        ),
        (
            "a name no rule carries",
            json!({ "ConfigRuleNames": ["absent"] }),
            None,
        ),
        (
            "a known name beside an unknown one",
            json!({ "ConfigRuleNames": ["a-rule", "absent"] }),
            None,
        ),
    ];

    for (case, request, expected) in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        for name in ["a-rule", "b-rule", "c-rule"] {
            emulator.ok("PutConfigRule", &managed_rule(name)).await;
        }
        emulator
            .ok(
                "PutConfigRule",
                &rule_owned_by("lambda-rule", OWNER_CUSTOM_LAMBDA),
            )
            .await;

        match expected {
            Some(names) => {
                let body = emulator
                    .ok("DescribeConfigRuleEvaluationStatus", &request)
                    .await;
                let reported = reported_names(case, &body);
                assert_eq!(
                    reported, names,
                    "case {case:?}: expected statuses for {names:?}, got {reported:?}"
                );
            }
            None => {
                let (status, code) = emulator
                    .refused("DescribeConfigRuleEvaluationStatus", &request)
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "NoSuchConfigRuleException"),
                    "case {case:?}: expected a 400 NoSuchConfigRuleException, got {status} \
                     {code}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_statuses_page_at_the_limit_the_request_asked_for() {
    // Rows differ along how many rules the account holds and the Limit the request
    // states: no limit at all (the modelled default of 0), one, a handful, and the
    // published maximum of 50 (`RuleLimit` range 0..50).
    let cases: [(&str, usize, u64); 7] = [
        ("no rules, no limit", 0, 0),
        ("one rule, no limit", 1, 0),
        ("a page and one, no limit", DEFAULT_PAGE_SIZE + 1, 0),
        ("one rule, a limit of one", 1, 1),
        ("five rules, a limit of one", 5, 1),
        ("eleven rules, a limit of four", 11, 4),
        ("eleven rules, the maximum limit", 11, 50),
    ];

    for (case, rules, limit) in cases {
        let emulator = Emulator::new();
        if rules > 0 {
            with_recorder_and_channel(&emulator).await;
        }
        let mut expected: Vec<String> = Vec::with_capacity(rules);
        for index in 0..rules {
            let name = format!("rule-{index:03}");
            emulator.ok("PutConfigRule", &managed_rule(&name)).await;
            expected.push(name);
        }
        expected.sort();

        let page_size = if limit == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            usize::try_from(limit).expect("a limit fits")
        };
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=(rules / page_size + 1) {
            let mut request = json!({});
            if limit > 0 {
                request["Limit"] = json!(limit);
            }
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let body = emulator
                .ok("DescribeConfigRuleEvaluationStatus", &request)
                .await;
            let names = reported_names(case, &body);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page must carry at most {page_size} statuses, got {}",
                names.len()
            );
            collected.extend(names);
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the pages must run out, got a further token {token:?}"
        );
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages must together carry every rule once, in name order"
        );
    }
}

#[tokio::test]
async fn a_limit_outside_the_published_range_is_refused() {
    // Rows differ along how the limit leaves the published `RuleLimit` range (0..50):
    // one past the maximum, far past it, and below the minimum. The two rows at the
    // bounds themselves are served, which the paging table above covers.
    let cases: [(&str, i64); 3] = [
        ("one past the maximum", 51),
        ("far past the maximum", 5_000),
        ("below the minimum", -1),
    ];

    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule("a-rule")).await;

    for (case, limit) in cases {
        let (status, code) = emulator
            .refused(
                "DescribeConfigRuleEvaluationStatus",
                &json!({ "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for Limit {limit}, got {status} {code}"
        );
        assert!(
            code == "InvalidParameterValueException" || code == "ValidationException",
            "case {case:?}: expected the modelled bad-parameter refusal for Limit \
             {limit}, got {code}"
        );
    }

    // A refusal must not have served a page either way.
    let served = emulator
        .ok("DescribeConfigRuleEvaluationStatus", &json!({}))
        .await;
    assert_eq!(
        reported_names("after the refusals", &served),
        ["a-rule"],
        "the rule must still be reported after the refused requests, got {served}"
    );
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    // Rows differ along how the token is malformed: not base64 at all, base64 of bytes
    // that are not a name, and the empty string.
    let cases: [(&str, &str); 3] = [
        ("not base64", "not-a-token!!"),
        ("base64 of invalid UTF-8", "/w=="),
        ("a bare name, unencoded", "rule-000 "),
    ];

    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for index in 0..3 {
        emulator
            .ok("PutConfigRule", &managed_rule(&format!("rule-{index:03}")))
            .await;
    }

    for (case, token) in cases {
        let (status, code) = emulator
            .refused(
                "DescribeConfigRuleEvaluationStatus",
                &json!({ "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected a 400 InvalidNextTokenException for {token:?}, got \
             {status} {code}"
        );
    }
}
