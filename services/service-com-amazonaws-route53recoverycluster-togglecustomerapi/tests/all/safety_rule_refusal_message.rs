// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of the refusal a deployed safety rule answers a
//! routing-control state update with.
//!
//! The refusal is the one an operator meets mid-failover, so its sentence is the thing
//! that tells them the way out: Route 53 ARC quotes the condition that does not hold,
//! points at the `AllowSafetyRulesOverride` guidance (which is what the request's
//! `SafetyRulesToOverride` does), and then names the routing control the request asked to
//! move and the rule that blocked it, each on its own labelled line. Everything else in
//! the sentence is fixed text.
//!
//! The suite asserts the WHOLE message, character for character, over a table of rules
//! whose stored `RuleConfig` differs along what the quoted condition is rendered from —
//! every value of the modelled `RuleType`, `Inverted` both ways, thresholds at, below and
//! past the number of controls a rule evaluates — plus both kinds of rule and both update
//! operations.

use super::common::{
    call, create_assertion_rule, create_cluster, create_control_panel, create_gating_rule,
    create_routing_control, emulator, error_code, error_message, Control, Panel, RuleConfigSpec,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A fresh emulator with one control panel holding `count` routing controls, all `Off`.
async fn panel_with_controls(count: usize) -> (MultiServiceRouter, Panel, Vec<Control>) {
    let router = emulator();
    let cluster = create_cluster(&router, "refusal-wording").await;
    let mut panel = create_control_panel(&router, &cluster, "refusal-wording-panel").await;
    let mut controls = Vec::new();
    for index in 0..count {
        let control = create_routing_control(&router, &panel, &format!("control-{index}")).await;
        controls.push(control.clone());
        panel.controls.push(control);
    }
    (router, panel, controls)
}

/// The message AWS states when `safety_rule_arn` blocks an update of
/// `routing_control_arns`, with `condition` the rule's condition as the service words it.
///
/// Spelled out here independently of the emulator's own builder, so a change to that
/// builder has to agree with this sentence rather than redefine it.
fn expected_message(condition: &str, routing_control_arns: &[&str], safety_rule_arn: &str) -> String {
    format!(
        "Can't update the routing control state because of a configured safety rule: \
         \"{condition}.\" To override the rule and change the state, see the guidance for \
         AllowSafetyRulesOverride in the Route 53 ARC Developer Guide. \nRouting control: \
         [{controls}] \nSafety rule: {safety_rule_arn}",
        controls = routing_control_arns.join(", "),
    )
}

/// Which kind of rule a row deploys, over which of the panel's controls (by index).
enum RuleSpec {
    /// An assertion rule over `asserted`.
    Assertion {
        asserted: &'static [usize],
        config: RuleConfigSpec,
    },
    /// A gating rule whose `gating` controls gate `targets`.
    Gating {
        gating: &'static [usize],
        targets: &'static [usize],
        config: RuleConfigSpec,
    },
}

/// One row: a rule, a fixture, the update it refuses, and the condition its refusal must
/// quote.
struct Row {
    label: &'static str,
    rule: RuleSpec,
    /// The states the panel's controls are driven to BEFORE the rule is deployed.
    initial: &'static [(usize, &'static str)],
    /// The update attempted once the rule is deployed, which the rule must refuse.
    update: (usize, &'static str),
    /// The clause the refusal must quote, rendered from the row's stored `RuleConfig`.
    condition: &'static str,
}

/// Drive one routing control's state, expecting the update to be ACCEPTED.
async fn set_state(router: &MultiServiceRouter, case: &str, arn: &str, state: &str) {
    let (status, body) = call(
        router,
        "UpdateRoutingControlState",
        &json!({ "RoutingControlArn": arn, "RoutingControlState": state }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: setting {arn} to {state} must be accepted, got {status} with {body}"
    );
}

/// Deploy the row's rule and answer its `SafetyRuleArn`.
async fn deploy(router: &MultiServiceRouter, panel: &Panel, controls: &[Control], rule: &RuleSpec) -> String {
    match rule {
        RuleSpec::Assertion { asserted, config } => {
            let asserted: Vec<String> = asserted.iter().map(|i| controls[*i].arn.clone()).collect();
            create_assertion_rule(router, &panel.arn, "blocking-rule", &asserted, *config).await
        }
        RuleSpec::Gating {
            gating,
            targets,
            config,
        } => {
            let gating: Vec<String> = gating.iter().map(|i| controls[*i].arn.clone()).collect();
            let targets: Vec<String> = targets.iter().map(|i| controls[*i].arn.clone()).collect();
            create_gating_rule(router, &panel.arn, "blocking-rule", &gating, &targets, *config).await
        }
    }
}

/// RULE: an update a deployed safety rule refuses is answered with HTTP 400
/// `ValidationException` whose message is Route 53 ARC's fixed sentence — the condition
/// the rule's stored `RuleConfig` imposes, quoted, the `AllowSafetyRulesOverride`
/// remediation, and the labelled `Routing control:` and `Safety rule:` lines carrying the
/// ARNs the request named and the rule that blocked it — and nothing else: no rule name,
/// no count of controls, no raw `RuleConfig` members.
///
/// The rows differ in what the quoted condition is rendered from: every value of the
/// modelled `RuleType` (`ATLEAST`, `AND`, `OR` — see
/// `smithy_query(query="doc",shape="RuleType")` on the control plane's model, which
/// defines `AND` as "at least N, where N is the total number of controls in the rule" and
/// `OR` as "at least N, where N is 1"), `Inverted` both ways, thresholds of 1, the number
/// of evaluated controls and one past it, and both kinds of rule.
#[tokio::test]
async fn a_refused_update_states_the_blocking_rule_condition_and_the_override_guidance() {
    let rows: Vec<Row> = vec![
        Row {
            label: "ATLEAST 3 over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "ATLEAST",
                    threshold: 3,
                    inverted: false,
                },
            },
            initial: &[(0, "On"), (1, "On")],
            update: (0, "On"),
            condition: "At least 3 routing controls must be On",
        },
        Row {
            label: "ATLEAST 2 over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "ATLEAST",
                    threshold: 2,
                    inverted: false,
                },
            },
            initial: &[(0, "On"), (1, "On")],
            update: (0, "Off"),
            condition: "At least 2 routing controls must be On",
        },
        Row {
            label: "ATLEAST 1 over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "ATLEAST",
                    threshold: 1,
                    inverted: false,
                },
            },
            initial: &[(0, "On")],
            update: (0, "Off"),
            condition: "At least 1 routing controls must be On",
        },
        Row {
            label: "inverted ATLEAST 2 over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "ATLEAST",
                    threshold: 2,
                    inverted: true,
                },
            },
            initial: &[(0, "On")],
            update: (1, "On"),
            condition: "Fewer than 2 routing controls must be On",
        },
        Row {
            label: "AND over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "AND",
                    threshold: 0,
                    inverted: false,
                },
            },
            initial: &[(0, "On"), (1, "On")],
            update: (0, "Off"),
            condition: "At least 2 routing controls must be On",
        },
        Row {
            label: "AND over 3 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1, 2],
                config: RuleConfigSpec {
                    rule_type: "AND",
                    threshold: 0,
                    inverted: false,
                },
            },
            initial: &[(0, "On"), (1, "On"), (2, "On")],
            update: (2, "Off"),
            condition: "At least 3 routing controls must be On",
        },
        Row {
            label: "OR over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "OR",
                    threshold: 0,
                    inverted: false,
                },
            },
            initial: &[(0, "On")],
            update: (0, "Off"),
            condition: "At least 1 routing controls must be On",
        },
        Row {
            label: "inverted OR over 2 asserted controls",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "OR",
                    threshold: 0,
                    inverted: true,
                },
            },
            initial: &[],
            update: (0, "On"),
            condition: "Fewer than 1 routing controls must be On",
        },
        Row {
            label: "a shut OR gate over 1 gating control",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: RuleConfigSpec {
                    rule_type: "OR",
                    threshold: 0,
                    inverted: false,
                },
            },
            initial: &[],
            update: (1, "On"),
            condition: "At least 1 routing controls must be On",
        },
        Row {
            label: "a shut AND gate over 2 gating controls",
            rule: RuleSpec::Gating {
                gating: &[0, 1],
                targets: &[2],
                config: RuleConfigSpec {
                    rule_type: "AND",
                    threshold: 0,
                    inverted: false,
                },
            },
            initial: &[(0, "On")],
            update: (2, "On"),
            condition: "At least 2 routing controls must be On",
        },
        Row {
            label: "an inverted OR gate over 1 gating control",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: RuleConfigSpec {
                    rule_type: "OR",
                    threshold: 0,
                    inverted: true,
                },
            },
            initial: &[(0, "On")],
            update: (1, "On"),
            condition: "Fewer than 1 routing controls must be On",
        },
    ];

    for row in &rows {
        let case = row.label;
        let (router, panel, controls) = panel_with_controls(3).await;

        // The fixture is driven into place BEFORE the rule is deployed, so the setup is
        // never the thing the rule refuses.
        for (index, state) in row.initial {
            set_state(&router, case, &controls[*index].arn, state).await;
        }
        let rule_arn = deploy(&router, &panel, &controls, &row.rule).await;

        let (index, requested) = row.update;
        let arn = controls[index].arn.clone();
        let (status, body) = call(
            &router,
            "UpdateRoutingControlState",
            &json!({ "RoutingControlArn": arn, "RoutingControlState": requested }),
        )
        .await;

        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        let expected = expected_message(row.condition, &[arn.as_str()], &rule_arn);
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case}: expected message {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: the batch operation words the same refusal the same way — the ARNs it names in
/// the `Routing control:` line are the ones its own entries carry, in request order.
#[tokio::test]
async fn the_batch_update_states_the_same_refusal() {
    let (router, panel, controls) = panel_with_controls(3).await;
    let arns: Vec<String> = controls.iter().map(|control| control.arn.clone()).collect();

    // "At least 3 of these two controls must be On", which no transaction can satisfy.
    let rule_arn = create_assertion_rule(
        &router,
        &panel.arn,
        "blocking-rule",
        &[arns[0].clone(), arns[1].clone()],
        RuleConfigSpec {
            rule_type: "ATLEAST",
            threshold: 3,
            inverted: false,
        },
    )
    .await;

    let cases: Vec<(&str, Vec<String>)> = vec![
        ("one entry", vec![arns[0].clone()]),
        ("two entries", vec![arns[0].clone(), arns[1].clone()]),
        // A control the rule does not name, batched with one it does: the request's own
        // entries are what the line reports, in the order they were sent.
        (
            "an entry the rule does not name, first",
            vec![arns[2].clone(), arns[1].clone()],
        ),
    ];

    for (case, entries) in &cases {
        let request = json!({
            "UpdateRoutingControlStateEntries": entries
                .iter()
                .map(|arn| json!({ "RoutingControlArn": arn, "RoutingControlState": "On" }))
                .collect::<Vec<Value>>(),
        });
        let (status, body) = call(&router, "UpdateRoutingControlStates", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        let named: Vec<&str> = entries.iter().map(String::as_str).collect();
        let expected = expected_message(
            "At least 3 routing controls must be On",
            &named,
            &rule_arn,
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case}: expected message {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: the wording change is wording only — a refused update still writes nothing, and
/// once the blocking rule is deleted the identical call is accepted and the state moves.
#[tokio::test]
async fn a_refused_update_still_writes_nothing_and_succeeds_once_the_rule_is_gone() {
    use super::common::{control_plane, encode_arn, get_state};

    let (router, panel, controls) = panel_with_controls(2).await;
    let arn = controls[0].arn.clone();

    let rule_arn = create_assertion_rule(
        &router,
        &panel.arn,
        "blocking-rule",
        &[arn.clone(), controls[1].arn.clone()],
        RuleConfigSpec {
            rule_type: "ATLEAST",
            threshold: 3,
            inverted: false,
        },
    )
    .await;

    let (status, body) = call(
        &router,
        "UpdateRoutingControlState",
        &json!({ "RoutingControlArn": arn, "RoutingControlState": "On" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the rule is deployed: expected 400, got {status} with {body}"
    );
    let reported = get_state(&router, &arn).await;
    assert_eq!(
        reported.get("RoutingControlState").and_then(Value::as_str),
        Some("Off"),
        "case the rule is deployed: a refused update must not move {arn}, got {reported}"
    );

    let (status, body) = control_plane(
        &router,
        "DELETE",
        &format!("/safetyrule/{}", encode_arn(&rule_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteSafetyRule: expected 200, got {status} with {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateRoutingControlState",
        &json!({ "RoutingControlArn": arn, "RoutingControlState": "On" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the rule is deleted: expected 200, got {status} with {body}"
    );
    let reported = get_state(&router, &arn).await;
    assert_eq!(
        reported.get("RoutingControlState").and_then(Value::as_str),
        Some("On"),
        "case the rule is deleted: the update must move {arn} to On, got {reported}"
    );
}
