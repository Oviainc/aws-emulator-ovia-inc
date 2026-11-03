// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the safety rules the Route 53 ARC cluster (data-plane) update
//! operations are gated by: `UpdateRoutingControlState` and
//! `UpdateRoutingControlStates` against assertion rules, gating rules and
//! `SafetyRulesToOverride`.
//!
//! Every call goes over the wire the real SDK uses — the data-plane calls as `POST /`
//! with `X-Amz-Target: ToggleCustomerAPI.<Op>` and an awsJson1_0 body, the control-plane
//! ones as the REST routes that create the resources — through the same
//! `MultiServiceRouter` the emulator boots. The safety rules are created through the
//! CONTROL plane, which is the only way one exists at all, and are waited to `DEPLOYED`
//! before they are expected to guard anything.
//!
//! The rule under test is asserted over a TABLE of inputs: both kinds of rule, every
//! value of the modelled `RuleType` enum, `Inverted` both ways, thresholds at and past
//! the number of controls a rule evaluates, a control no rule guards, and the single and
//! the batch update operation.

use serde_json::{json, Value};

use super::common::{
    call, create_assertion_rule, create_control_panel, create_cluster, create_gating_rule,
    create_routing_control, emulator, error_code, error_message, get_state, Control, Panel,
    RuleConfigSpec,
};
use smithy_server_runtime::MultiServiceRouter;

/// A fresh emulator with one control panel holding `count` routing controls, all `Off`:
/// the fixture every row below deploys its own rule into.
async fn panel_with_controls(count: usize) -> (MultiServiceRouter, Panel, Vec<Control>) {
    let router = emulator();
    let cluster = create_cluster(&router, "safety-rules").await;
    let mut panel = create_control_panel(&router, &cluster, "safety-rules-panel").await;
    let mut controls = Vec::new();
    for index in 0..count {
        let control = create_routing_control(&router, &panel, &format!("control-{index}")).await;
        controls.push(control.clone());
        panel.controls.push(control);
    }
    (router, panel, controls)
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

/// The state `GetRoutingControlState` reports for `arn`.
async fn reported_state(router: &MultiServiceRouter, arn: &str) -> String {
    get_state(router, arn)
        .await
        .get("RoutingControlState")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `UpdateRoutingControlState`, optionally overriding safety rules, as `(status, body)`.
async fn update(
    router: &MultiServiceRouter,
    arn: &str,
    state: &str,
    overrides: &[String],
) -> (u16, Value) {
    let mut request = serde_json::Map::new();
    request.insert("RoutingControlArn".to_string(), json!(arn));
    request.insert("RoutingControlState".to_string(), json!(state));
    if !overrides.is_empty() {
        request.insert("SafetyRulesToOverride".to_string(), json!(overrides));
    }
    call(router, "UpdateRoutingControlState", &Value::Object(request)).await
}

/// `UpdateRoutingControlStates`, optionally overriding safety rules, as `(status, body)`.
async fn update_batch(
    router: &MultiServiceRouter,
    entries: &[(String, &str)],
    overrides: &[String],
) -> (u16, Value) {
    let mut request = serde_json::Map::new();
    request.insert(
        "UpdateRoutingControlStateEntries".to_string(),
        json!(entries
            .iter()
            .map(|(arn, state)| json!({
                "RoutingControlArn": arn,
                "RoutingControlState": state,
            }))
            .collect::<Vec<Value>>()),
    );
    if !overrides.is_empty() {
        request.insert("SafetyRulesToOverride".to_string(), json!(overrides));
    }
    call(router, "UpdateRoutingControlStates", &Value::Object(request)).await
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

/// One row of the enforcement table.
struct Row {
    label: &'static str,
    rule: RuleSpec,
    /// The states the panel's controls are driven to BEFORE the rule is deployed.
    initial: &'static [(usize, &'static str)],
    /// The update attempted once the rule is deployed.
    update: (usize, &'static str),
    /// Whether the deployed rule must refuse that update.
    refused: bool,
}

/// RULE: a deployed safety rule of a routing control's control panel is evaluated before
/// the control's state is written — an assertion rule over the state its
/// `AssertedControls` would be left in, a gating rule over the state its `GatingControls`
/// would be left in whenever the update touches one of its `TargetControls` — and an
/// update that would leave that rule's `RuleConfig` unsatisfied is refused with HTTP 400
/// `ValidationException` and writes nothing.
///
/// The rows span both kinds of rule, every value of the modelled `RuleType` enum
/// (`ATLEAST`, `AND`, `OR` — see `smithy_query(query="doc",shape="RuleType")` on the
/// control plane's model, which defines `AND` as "at least N, where N is the total number
/// of controls in the rule" and `OR` as "at least N, where N is 1"), `Inverted` both
/// ways, thresholds of 0, 1, the number of evaluated controls and one past it, an update
/// that SATISFIES each rule, an update to a gating control (which its own rule does not
/// gate) and an update to a control no rule names.
#[tokio::test]
async fn a_deployed_safety_rule_gates_the_updates_it_guards() {
    const ATLEAST_ONE: RuleConfigSpec = RuleConfigSpec {
        rule_type: "ATLEAST",
        threshold: 1,
        inverted: false,
    };
    const AND_ALL: RuleConfigSpec = RuleConfigSpec {
        rule_type: "AND",
        threshold: 2,
        inverted: false,
    };
    const ANY_ON: RuleConfigSpec = RuleConfigSpec {
        rule_type: "OR",
        threshold: 0,
        inverted: false,
    };
    const NONE_ON: RuleConfigSpec = RuleConfigSpec {
        rule_type: "OR",
        threshold: 0,
        inverted: true,
    };

    let rows: Vec<Row> = vec![
        Row {
            label: "ATLEAST 1 of 2 asserted: switching the last On control off",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: ATLEAST_ONE,
            },
            initial: &[(0, "On")],
            update: (0, "Off"),
            refused: true,
        },
        Row {
            label: "ATLEAST 1 of 2 asserted: switching the other control on",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: ATLEAST_ONE,
            },
            initial: &[(0, "On")],
            update: (1, "On"),
            refused: false,
        },
        Row {
            label: "ATLEAST 1 of 2 asserted: switching one of two On controls off",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: ATLEAST_ONE,
            },
            initial: &[(0, "On"), (1, "On")],
            update: (0, "Off"),
            refused: false,
        },
        Row {
            label: "ATLEAST 0 of 2 asserted: a threshold nothing can fall below",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: RuleConfigSpec {
                    rule_type: "ATLEAST",
                    threshold: 0,
                    inverted: false,
                },
            },
            initial: &[(0, "On")],
            update: (0, "Off"),
            refused: false,
        },
        Row {
            label: "ATLEAST 3 of 2 asserted: a threshold past every control it evaluates",
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
            refused: true,
        },
        Row {
            label: "AND over 2 asserted: switching one of them off",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: AND_ALL,
            },
            initial: &[(0, "On"), (1, "On")],
            update: (0, "Off"),
            refused: true,
        },
        Row {
            label: "AND over 2 asserted: switching the missing one on",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: AND_ALL,
            },
            initial: &[(1, "On")],
            update: (0, "On"),
            refused: false,
        },
        Row {
            label: "OR over 2 asserted: switching the last On control off",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: ANY_ON,
            },
            initial: &[(0, "On")],
            update: (0, "Off"),
            refused: true,
        },
        Row {
            label: "inverted OR over 2 asserted: switching a control on",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: NONE_ON,
            },
            initial: &[],
            update: (0, "On"),
            refused: true,
        },
        Row {
            label: "inverted OR over 2 asserted: switching the On control off",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: NONE_ON,
            },
            initial: &[(0, "On")],
            update: (0, "Off"),
            refused: false,
        },
        Row {
            label: "ATLEAST 1 of 2 asserted: a control the rule does not name",
            rule: RuleSpec::Assertion {
                asserted: &[0, 1],
                config: ATLEAST_ONE,
            },
            initial: &[],
            update: (2, "On"),
            refused: false,
        },
        Row {
            label: "OR gate shut: switching the target on",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: ANY_ON,
            },
            initial: &[],
            update: (1, "On"),
            refused: true,
        },
        Row {
            label: "OR gate shut: switching the target off",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: ANY_ON,
            },
            initial: &[(1, "On")],
            update: (1, "Off"),
            refused: true,
        },
        Row {
            label: "OR gate shut: switching the GATE itself on",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: ANY_ON,
            },
            initial: &[],
            update: (0, "On"),
            refused: false,
        },
        Row {
            label: "OR gate open: switching the target on",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: ANY_ON,
            },
            initial: &[(0, "On")],
            update: (1, "On"),
            refused: false,
        },
        Row {
            label: "AND over 2 gates, one shut: switching the target on",
            rule: RuleSpec::Gating {
                gating: &[0, 1],
                targets: &[2],
                config: AND_ALL,
            },
            initial: &[(0, "On")],
            update: (2, "On"),
            refused: true,
        },
        Row {
            label: "AND over 2 gates, both open: switching the target on",
            rule: RuleSpec::Gating {
                gating: &[0, 1],
                targets: &[2],
                config: AND_ALL,
            },
            initial: &[(0, "On"), (1, "On")],
            update: (2, "On"),
            refused: false,
        },
        Row {
            label: "inverted OR gate: switching the target on while a gate is On",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: NONE_ON,
            },
            initial: &[(0, "On")],
            update: (1, "On"),
            refused: true,
        },
        Row {
            label: "inverted OR gate: switching the target on while every gate is Off",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: NONE_ON,
            },
            initial: &[],
            update: (1, "On"),
            refused: false,
        },
        Row {
            label: "OR gate shut: a control that is neither gate nor target",
            rule: RuleSpec::Gating {
                gating: &[0],
                targets: &[1],
                config: ANY_ON,
            },
            initial: &[],
            update: (2, "On"),
            refused: false,
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

        let rule_arn = match &row.rule {
            RuleSpec::Assertion { asserted, config } => {
                let asserted: Vec<String> =
                    asserted.iter().map(|i| controls[*i].arn.clone()).collect();
                create_assertion_rule(&router, &panel.arn, "rule", &asserted, *config).await
            }
            RuleSpec::Gating {
                gating,
                targets,
                config,
            } => {
                let gating: Vec<String> =
                    gating.iter().map(|i| controls[*i].arn.clone()).collect();
                let targets: Vec<String> =
                    targets.iter().map(|i| controls[*i].arn.clone()).collect();
                create_gating_rule(&router, &panel.arn, "rule", &gating, &targets, *config).await
            }
        };

        let (index, requested) = row.update;
        let arn = &controls[index].arn;
        let before = reported_state(&router, arn).await;
        let (status, body) = update(&router, arn, requested, &[]).await;

        if row.refused {
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            // The refusal names the rule that blocked it: that ARN is what a caller puts
            // in `SafetyRulesToOverride`.
            assert!(
                error_message(&body).contains(&rule_arn),
                "case {case}: the refusal must name the safety rule {rule_arn}, got {body}"
            );
            let after = reported_state(&router, arn).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must not move {arn}, got {after}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {body}"
            );
            let after = reported_state(&router, arn).await;
            assert_eq!(
                after, requested,
                "case {case}: an accepted update must move {arn} to {requested}, got {after}"
            );
        }
    }
}

/// RULE: a safety rule whose ARN the request lists in `SafetyRulesToOverride` is left out
/// of the evaluation for that one call, so the update it would otherwise refuse is
/// accepted and the state really moves; every rule the request does NOT name is still
/// evaluated and still refuses.
///
/// The rows are every subset of two deployed rules that the same update violates:
/// neither overridden, each one overridden alone, both overridden — plus an ARN that
/// names no rule at all.
#[tokio::test]
async fn an_overridden_safety_rule_is_not_evaluated_for_that_call() {
    let (router, panel, controls) = panel_with_controls(2).await;
    set_state(&router, "setup", &controls[0].arn, "On").await;

    // Two rules the same update (control 0 -> Off) violates: "at least one of the two
    // must stay On" and "control 0 itself must be On".
    let both = create_assertion_rule(
        &router,
        &panel.arn,
        "at-least-one-of-both",
        &[controls[0].arn.clone(), controls[1].arn.clone()],
        RuleConfigSpec {
            rule_type: "ATLEAST",
            threshold: 1,
            inverted: false,
        },
    )
    .await;
    let first_only = create_assertion_rule(
        &router,
        &panel.arn,
        "first-must-be-on",
        &[controls[0].arn.clone()],
        RuleConfigSpec {
            rule_type: "AND",
            threshold: 1,
            inverted: false,
        },
    )
    .await;
    let unknown_rule = format!("{}/safetyrule/00000000-0000-0000-0000-000000000000", panel.arn);

    let refused: Vec<(&str, Vec<String>)> = vec![
        ("no override at all", vec![]),
        ("only the two-control rule overridden", vec![both.clone()]),
        ("only the one-control rule overridden", vec![first_only.clone()]),
        ("a rule ARN that names no rule", vec![unknown_rule]),
    ];

    for (case, overrides) in &refused {
        let (status, body) = update(&router, &controls[0].arn, "Off", overrides).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        let state = reported_state(&router, &controls[0].arn).await;
        assert_eq!(
            state, "On",
            "case {case}: a refused update must leave the control On, got {state}"
        );
    }

    // Both rules overridden: the break-glass change goes through and the control really
    // moves.
    let overrides = vec![both.clone(), first_only.clone()];
    let (status, body) = update(&router, &controls[0].arn, "Off", &overrides).await;
    assert_eq!(
        status, 200,
        "case both rules overridden: expected 200, got {status} with {body}"
    );
    let state = reported_state(&router, &controls[0].arn).await;
    assert_eq!(
        state, "Off",
        "case both rules overridden: the control must have moved to Off, got {state}"
    );

    // An override is per-call, not a change to the rules: once the control is restored,
    // the same violating update is refused again when nothing is overridden.
    let (status, body) = update(&router, &controls[0].arn, "On", &[]).await;
    assert_eq!(
        status, 200,
        "case restoring the first control: expected 200, got {status} with {body}"
    );
    let state = reported_state(&router, &controls[0].arn).await;
    assert_eq!(
        state, "On",
        "case restoring the first control: expected On, got {state}"
    );
    let (status, body) = update(&router, &controls[0].arn, "Off", &[]).await;
    assert_eq!(
        status, 400,
        "case the same update without the override: expected 400, got {status} with {body}"
    );
    let state = reported_state(&router, &controls[0].arn).await;
    assert_eq!(
        state, "On",
        "case the same update without the override: expected On, got {state}"
    );
}

/// RULE: `UpdateRoutingControlStates` is one transaction, so a deployed rule is evaluated
/// over the state the WHOLE entry set would leave its controls in — a batch can satisfy a
/// rule no single entry of it could — and a batch a rule refuses commits no entry at all.
#[tokio::test]
async fn a_batch_update_is_gated_as_one_transaction() {
    let (router, panel, controls) = panel_with_controls(3).await;
    let arns: Vec<String> = controls.iter().map(|c| c.arn.clone()).collect();

    // "Both of these two controls must be On after the transaction."
    let rule = create_assertion_rule(
        &router,
        &panel.arn,
        "both-on",
        &[arns[0].clone(), arns[1].clone()],
        RuleConfigSpec {
            rule_type: "AND",
            threshold: 2,
            inverted: false,
        },
    )
    .await;

    // One entry on its own cannot satisfy the rule, and commits nothing.
    let (status, body) = update_batch(&router, &[(arns[0].clone(), "On")], &[]).await;
    assert_eq!(
        status, 400,
        "case one entry of two: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case one entry of two: expected ValidationException, got {body}"
    );
    for arn in &arns {
        let state = reported_state(&router, arn).await;
        assert_eq!(
            state, "Off",
            "case one entry of two: a refused batch must commit nothing, got {state} for {arn}"
        );
    }

    // The same two writes in ONE batch satisfy the rule, and both land.
    let (status, body) = update_batch(
        &router,
        &[(arns[0].clone(), "On"), (arns[1].clone(), "On")],
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case both entries together: expected 200, got {status} with {body}"
    );
    for arn in &arns[..2] {
        let state = reported_state(&router, arn).await;
        assert_eq!(
            state, "On",
            "case both entries together: every entry must be applied, got {state} for {arn}"
        );
    }

    // A control the rule does not name is not gated by it.
    let (status, body) = update_batch(&router, &[(arns[2].clone(), "On")], &[]).await;
    assert_eq!(
        status, 200,
        "case a control no rule names: expected 200, got {status} with {body}"
    );

    // Taking both controls back down violates the rule, and neither entry is committed.
    let down = [(arns[0].clone(), "Off"), (arns[1].clone(), "Off")];
    let (status, body) = update_batch(&router, &down, &[]).await;
    assert_eq!(
        status, 400,
        "case both entries down: expected 400, got {status} with {body}"
    );
    for arn in &arns[..2] {
        let state = reported_state(&router, arn).await;
        assert_eq!(
            state, "On",
            "case both entries down: a refused batch must commit nothing, got {state} for {arn}"
        );
    }

    // Overriding the rule lets the same batch through, and both entries land.
    let (status, body) = update_batch(&router, &down, std::slice::from_ref(&rule)).await;
    assert_eq!(
        status, 200,
        "case both entries down with the rule overridden: expected 200, got {status} with {body}"
    );
    for arn in &arns[..2] {
        let state = reported_state(&router, arn).await;
        assert_eq!(
            state, "Off",
            "case both entries down with the rule overridden: expected Off, got {state} for {arn}"
        );
    }
}

/// RULE: a gating rule orders the two updates — while the gate does not satisfy the rule
/// the target cannot be moved, the gate itself is not gated by its own rule, and once the
/// gate satisfies it the identical target update is accepted.
#[tokio::test]
async fn a_gating_rule_orders_the_gate_before_its_target() {
    let (router, panel, controls) = panel_with_controls(2).await;
    let gate = controls[0].arn.clone();
    let target = controls[1].arn.clone();

    let rule = create_gating_rule(
        &router,
        &panel.arn,
        "gate-before-target",
        std::slice::from_ref(&gate),
        std::slice::from_ref(&target),
        RuleConfigSpec {
            rule_type: "OR",
            threshold: 0,
            inverted: false,
        },
    )
    .await;

    // Beat 1: the gate is Off, so the target cannot be moved.
    let (status, body) = update(&router, &target, "On", &[]).await;
    assert_eq!(
        status, 400,
        "case the gate is shut: expected 400, got {status} with {body}"
    );
    assert!(
        error_message(&body).contains(&rule),
        "case the gate is shut: the refusal must name the gating rule {rule}, got {body}"
    );
    let state = reported_state(&router, &target).await;
    assert_eq!(
        state, "Off",
        "case the gate is shut: the target must not have moved, got {state}"
    );

    // Beat 2: the gate is not a target of its own rule, so opening it is accepted.
    set_state(&router, "opening the gate", &gate, "On").await;
    let state = reported_state(&router, &gate).await;
    assert_eq!(
        state, "On",
        "case opening the gate: the gate must report On, got {state}"
    );

    // Beat 3: the identical target update is now accepted.
    let (status, body) = update(&router, &target, "On", &[]).await;
    assert_eq!(
        status, 200,
        "case the gate is open: expected 200, got {status} with {body}"
    );
    let state = reported_state(&router, &target).await;
    assert_eq!(
        state, "On",
        "case the gate is open: the target must report On, got {state}"
    );

    // Beat 4: shutting the gate again is accepted, and gates the target once more.
    set_state(&router, "shutting the gate", &gate, "Off").await;
    let (status, body) = update(&router, &target, "Off", &[]).await;
    assert_eq!(
        status, 400,
        "case the gate is shut again: expected 400, got {status} with {body}"
    );

    // Beat 5: one batch that opens the gate and moves the target is one transaction, so
    // the rule reads the state the gate would be left in.
    let (status, body) = update_batch(
        &router,
        &[(gate.clone(), "On"), (target.clone(), "Off")],
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case gate and target in one batch: expected 200, got {status} with {body}"
    );
    for (arn, expected) in [(&gate, "On"), (&target, "Off")] {
        let state = reported_state(&router, arn).await;
        assert_eq!(
            state, expected,
            "case gate and target in one batch: expected {expected} for {arn}, got {state}"
        );
    }
}
