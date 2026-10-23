// SPDX-License-Identifier: CC0-1.0

//! What `StartConfigRulesEvaluation` does to the rules a request names.
//!
//! Four rules are pinned here, each over a table of inputs rather than one call:
//!
//! 1. the call invokes an evaluation pass for EXACTLY the rules it names — every named
//!    rule's invocation-and-evaluation instants move forward once the pass completes,
//!    and no rule the request left out moves at all;
//! 2. the pass is in flight when the call returns and completes on its own a span
//!    later, so a caller reading straight after the call still sees the previous
//!    pass's instants;
//! 3. a request that names a rule the account does not hold is refused, and a refused
//!    request evaluates nothing;
//! 4. a second on-demand call for a rule whose on-demand pass is still running is
//!    refused, and the same call is accepted once that pass has completed.
//!
//! The harness (`Emulator`, `managed_rule`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds, and every case reads the rules back
//! through `DescribeConfigRuleEvaluationStatus` — the operation that reports what this
//! one wrote.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// Longer than the span the clock owes an evaluation pass (500 ms), so a read after it
/// must see the completed pass.
const PAST_THE_PASS: Duration = Duration::from_millis(700);

/// The most names `ConfigRuleNames` may carry, from
/// `smithy_query(query="operation", operation="StartConfigRulesEvaluation")`:
/// `ReevaluateConfigRuleNames`, length 1..25.
const MAX_RULE_NAMES: usize = 25;

/// The instants `DescribeConfigRuleEvaluationStatus` reports for every rule that has
/// one, keyed by rule name: `(LastSuccessfulInvocationTime, LastSuccessfulEvaluationTime)`.
async fn evaluation_instants(emulator: &Emulator) -> BTreeMap<String, (Option<f64>, Option<f64>)> {
    let mut collected = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = emulator
            .ok("DescribeConfigRuleEvaluationStatus", &request)
            .await;
        for status in body["ConfigRulesEvaluationStatus"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a ConfigRulesEvaluationStatus list, got {body}"))
        {
            let name = status["ConfigRuleName"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a ConfigRuleName in {status}"))
                .to_string();
            collected.insert(
                name,
                (
                    status.get("LastSuccessfulInvocationTime").and_then(Value::as_f64),
                    status.get("LastSuccessfulEvaluationTime").and_then(Value::as_f64),
                ),
            );
        }
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
}

/// An emulator holding one managed rule per name in `names`, with the pass each write
/// owes already completed, plus the instants those passes recorded.
async fn with_settled_rules(names: &[String]) -> (Emulator, BTreeMap<String, (Option<f64>, Option<f64>)>) {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for name in names {
        emulator.ok("PutConfigRule", &managed_rule(name)).await;
    }
    tokio::time::sleep(PAST_THE_PASS).await;
    let settled = evaluation_instants(&emulator).await;
    for name in names {
        let instants = settled
            .get(name)
            .unwrap_or_else(|| panic!("{name} must report a status, got {settled:?}"));
        assert!(
            instants.0.is_some() && instants.1.is_some(),
            "the pass the write owes {name} must have completed, got {instants:?}"
        );
    }
    (emulator, settled)
}

/// `count` rule names, `rule-000`, `rule-001`, …
fn rule_names(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("rule-{index:03}")).collect()
}

#[tokio::test]
async fn an_on_demand_evaluation_moves_exactly_the_rules_the_request_names() {
    // Rows differ along WHICH names the request carries and how many: one of several,
    // a subset, every rule, the same name twice, and the published maximum of 25
    // names.
    struct Case {
        label: &'static str,
        rules: usize,
        named: Vec<String>,
    }

    let cases = [
        Case {
            label: "one rule, named",
            rules: 1,
            named: rule_names(1),
        },
        Case {
            label: "one of three rules",
            rules: 3,
            named: vec!["rule-001".to_string()],
        },
        Case {
            label: "two of four rules, named out of order",
            rules: 4,
            named: vec!["rule-003".to_string(), "rule-000".to_string()],
        },
        Case {
            label: "every rule the account holds",
            rules: 3,
            named: rule_names(3),
        },
        Case {
            // One call that names a rule twice evaluates it once: the two occurrences
            // share the one pass, so its completion instant is the one instant that
            // moves (a second pass would have been refused as already in progress).
            label: "the same name twice",
            rules: 2,
            named: vec!["rule-001".to_string(), "rule-001".to_string()],
        },
        Case {
            label: "the published maximum of names",
            rules: MAX_RULE_NAMES,
            named: rule_names(MAX_RULE_NAMES),
        },
    ];

    for case in cases {
        let label = case.label;
        let all = rule_names(case.rules);
        let (emulator, before) = with_settled_rules(&all).await;

        let started = emulator
            .ok(
                "StartConfigRulesEvaluation",
                &json!({ "ConfigRuleNames": case.named }),
            )
            .await;
        assert!(
            started.is_null() || started.as_object().is_some_and(serde_json::Map::is_empty),
            "case {label:?}: the response carries no members, got {started}"
        );

        tokio::time::sleep(PAST_THE_PASS).await;
        let after = evaluation_instants(&emulator).await;
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>(),
            "case {label:?}: an evaluation creates and deletes no rule"
        );

        for name in &all {
            let (was_invoked, was_evaluated) = before[name];
            let (invoked, evaluated) = after[name];
            if case.named.contains(name) {
                assert!(
                    invoked > was_invoked && evaluated > was_evaluated,
                    "case {label:?}: the pass this call owes {name} must move its instants \
                     past {was_invoked:?}/{was_evaluated:?}, got {invoked:?}/{evaluated:?}"
                );
                assert!(
                    invoked <= evaluated,
                    "case {label:?}: {name} must be invoked no later than it finished \
                     evaluating, got {invoked:?} then {evaluated:?}"
                );
            } else {
                assert_eq!(
                    (invoked, evaluated),
                    (was_invoked, was_evaluated),
                    "case {label:?}: {name} was not named, so its instants must not move"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_pass_the_call_starts_completes_on_its_own_a_span_later() {
    // Rows differ along how many of the account's rules the call names: one, and all
    // three of them.
    let cases: [(&str, &[&str]); 2] = [
        ("one rule named", &["rule-001"]),
        ("every rule named", &["rule-000", "rule-001", "rule-002"]),
    ];

    for (label, named) in cases {
        let all = rule_names(3);
        let (emulator, before) = with_settled_rules(&all).await;

        emulator
            .ok(
                "StartConfigRulesEvaluation",
                &json!({ "ConfigRuleNames": named }),
            )
            .await;

        // The pass is in flight: the rule still reports the previous pass's instants.
        let in_flight = evaluation_instants(&emulator).await;
        for name in named {
            assert_eq!(
                in_flight[*name], before[*name],
                "case {label:?}: the pass in flight must not move {name}'s instants yet, \
                 got {:?}",
                in_flight[*name]
            );
        }

        // Reading twice must report the same thing: the read reports state.
        let again = evaluation_instants(&emulator).await;
        assert_eq!(
            again, in_flight,
            "case {label:?}: a second read must report the same instants, got {again:?}"
        );

        // The completion is owed by the clock, not by the read.
        tokio::time::sleep(PAST_THE_PASS).await;
        let settled = evaluation_instants(&emulator).await;
        for name in named {
            assert!(
                settled[*name].1 > before[*name].1,
                "case {label:?}: the pass must have completed for {name}, got {:?} after \
                 {:?}",
                settled[*name],
                before[*name]
            );
        }
    }
}

#[tokio::test]
async fn a_request_this_operation_cannot_serve_is_refused_and_evaluates_nothing() {
    // Rows differ along what makes the request unserviceable: a name no rule carries,
    // such a name beside a known one, no names at all, an empty list, and one name
    // past the published maximum of 25.
    let over_the_maximum = rule_names(MAX_RULE_NAMES + 1);
    let cases: [(&str, Value, &[&str]); 5] = [
        (
            "a name no rule carries",
            json!({ "ConfigRuleNames": ["absent"] }),
            &["NoSuchConfigRuleException"],
        ),
        (
            "a known name beside an unknown one",
            json!({ "ConfigRuleNames": ["rule-000", "absent"] }),
            &["NoSuchConfigRuleException"],
        ),
        (
            // The handler itself decides this one: an omitted member is not a
            // modelled-constraint failure, so the code is pinned exactly.
            "no names at all",
            json!({}),
            &["InvalidParameterValueException"],
        ),
        (
            "an empty list of names",
            json!({ "ConfigRuleNames": [] }),
            &["InvalidParameterValueException", "ValidationException"],
        ),
        (
            "one name past the maximum",
            json!({ "ConfigRuleNames": over_the_maximum }),
            &["InvalidParameterValueException", "ValidationException"],
        ),
    ];

    for (label, request, expected) in cases {
        let all = rule_names(3);
        let (emulator, before) = with_settled_rules(&all).await;

        let (status, code) = emulator
            .refused("StartConfigRulesEvaluation", &request)
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a 400, got {status} {code}"
        );
        assert!(
            expected.contains(&code.as_str()),
            "case {label:?}: expected one of {expected:?}, got {code}"
        );

        // A refused request started no pass, now or a span later.
        tokio::time::sleep(PAST_THE_PASS).await;
        let after = evaluation_instants(&emulator).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused request must evaluate nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_second_call_is_refused_until_the_running_pass_completes() {
    // Rows differ along how the two calls overlap: the same single rule twice, a rule
    // named by both calls beside one named by neither, and two calls that share no
    // rule at all (which is no overlap, so the second is served).
    struct Case {
        label: &'static str,
        first: &'static [&'static str],
        second: &'static [&'static str],
        refused: bool,
    }

    let cases = [
        Case {
            label: "the same rule twice",
            first: &["rule-000"],
            second: &["rule-000"],
            refused: true,
        },
        Case {
            label: "a rule both calls name",
            first: &["rule-000", "rule-001"],
            second: &["rule-001", "rule-002"],
            refused: true,
        },
        Case {
            label: "two calls that share no rule",
            first: &["rule-000"],
            second: &["rule-002"],
            refused: false,
        },
    ];

    for case in cases {
        let label = case.label;
        let all = rule_names(3);
        let (emulator, before) = with_settled_rules(&all).await;

        emulator
            .ok(
                "StartConfigRulesEvaluation",
                &json!({ "ConfigRuleNames": case.first }),
            )
            .await;

        let second = json!({ "ConfigRuleNames": case.second });
        if case.refused {
            let (status, code) = emulator.refused("StartConfigRulesEvaluation", &second).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "LimitExceededException"),
                "case {label:?}: a pass this operation started is still running, so the \
                 second call must be refused, got {status} {code}"
            );
        } else {
            emulator.ok("StartConfigRulesEvaluation", &second).await;
        }

        // Once the running pass has completed the same call is served, and the rules
        // it names have moved.
        tokio::time::sleep(PAST_THE_PASS).await;
        emulator.ok("StartConfigRulesEvaluation", &second).await;
        tokio::time::sleep(PAST_THE_PASS).await;
        let after = evaluation_instants(&emulator).await;
        for name in case.first.iter().chain(case.second.iter()) {
            assert!(
                after[*name].1 > before[*name].1,
                "case {label:?}: {name} was evaluated on demand, so its instants must have \
                 moved past {:?}, got {:?}",
                before[*name],
                after[*name]
            );
        }
        for name in &all {
            if case.first.contains(&name.as_str()) || case.second.contains(&name.as_str()) {
                continue;
            }
            assert_eq!(
                after[name], before[name],
                "case {label:?}: {name} was named by neither call, so its instants must not \
                 move"
            );
        }
    }
}
