// SPDX-License-Identifier: CC0-1.0

//! `CreateSafetyRule`'s rule choice, over HTTP: a request carries EXACTLY ONE of the two
//! rule members, and a refused create leaves the control panel exactly as it was.
//!
//! The rules under test, each asserted over a table that spans what it generalises over
//! rather than the one request that exposed it:
//!
//! * the two rule members of a create are the alternatives of ONE choice: a request
//!   carrying both, or neither, is refused with `ValidationException` (400) and one fixed
//!   sentence that interpolates no part of the request — whatever else the request
//!   carries, and whether or not the members it carries would be valid on their own;
//! * a refused create persists NOTHING: the control panel's safety rules are the same
//!   after the refusal as before it, and the refusal consumes neither the request's
//!   `ClientToken` nor the name it asked for;
//! * a request carrying exactly one member succeeds, answers under that member alone, and
//!   the created rule appears in the control panel's list under its own kind key with the
//!   submitted name.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, enc, error_code, panel_with_controls, router};

/// The one sentence a create whose rule choice is wrong is refused with. It is fixed text:
/// no rule name, control-panel ARN or member name is interpolated into it.
const INVALID_RULE: &str = "Invalid Rule provided.";

/// The values `RuleType` declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="RuleType")`).
const RULE_TYPES: &[&str] = &["ATLEAST", "AND", "OR"];

/// An id in the 8-4-4-4-12 form these resources carry, which nothing in the store holds.
const UNSTORED_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ValidationException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// A `RuleConfig` body.
fn rule_config(rule_type: &str, inverted: bool, threshold: i32) -> Value {
    json!({ "Inverted": inverted, "Threshold": threshold, "Type": rule_type })
}

/// A `NewAssertionRule` body over `controls`.
fn assertion_member(
    panel_arn: &str,
    controls: &[String],
    name: &str,
    rule_type: &str,
    wait_period_ms: i32,
) -> Value {
    json!({
        "AssertedControls": controls,
        "ControlPanelArn": panel_arn,
        "Name": name,
        "RuleConfig": rule_config(rule_type, false, controls.len() as i32),
        "WaitPeriodMs": wait_period_ms
    })
}

/// A `NewGatingRule` body gating `gating_controls` over `target_controls`.
fn gating_member(
    panel_arn: &str,
    gating_controls: &[String],
    target_controls: &[String],
    name: &str,
    rule_type: &str,
    wait_period_ms: i32,
) -> Value {
    json!({
        "ControlPanelArn": panel_arn,
        "GatingControls": gating_controls,
        "Name": name,
        "RuleConfig": rule_config(rule_type, false, gating_controls.len() as i32),
        "TargetControls": target_controls,
        "WaitPeriodMs": wait_period_ms
    })
}

/// Send one `CreateSafetyRule` request and answer `(status, body)`.
async fn create_rule(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/safetyrule", Some(body)).await
}

/// The safety rules a control panel holds, as `(kind key, Name)` in list order — the
/// observable state a refused create must leave untouched.
async fn listed(router: &MultiServiceRouter, panel_arn: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "GET",
        &format!(
            "/controlpanel/{}/safetyrules?MaxResults=1000",
            enc(panel_arn)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list {panel_arn}: expected 200, got {status} {body:?}"
    );
    body["SafetyRules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            let kind = if rule["ASSERTION"].is_object() {
                "ASSERTION"
            } else if rule["GATING"].is_object() {
                "GATING"
            } else {
                "NEITHER"
            };
            (
                kind.to_string(),
                rule[kind]["Name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The ARN of a control panel this service does not hold, in the shape of a real one.
fn unstored_panel(panel_arn: &str) -> String {
    let (prefix, _) = panel_arn
        .rsplit_once('/')
        .expect("a control panel ARN names its resource after a '/'");
    format!("{prefix}/{UNSTORED_ID}")
}

/// One refused request: what makes its rule choice wrong, and the body that carries it.
struct Refused {
    case: String,
    body: Value,
}

/// The requests whose rule choice is wrong: neither member set, and both set — spanning
/// what else the request carries and whether the members would stand on their own.
fn refused_rows(panel_arn: &str, controls: &[String]) -> Vec<Refused> {
    let one = controls[..1].to_vec();
    let two = controls[..2].to_vec();
    let absent = unstored_panel(panel_arn);
    let unstored_control = format!("{panel_arn}/routingcontrol/{UNSTORED_ID}");

    let mut rows = vec![
        Refused {
            case: "neither member: an otherwise empty request".to_string(),
            body: json!({}),
        },
        Refused {
            case: "neither member: only Tags".to_string(),
            body: json!({ "Tags": { "suite": "rule-choice" } }),
        },
        Refused {
            case: "neither member: only a ClientToken".to_string(),
            body: json!({ "ClientToken": "rule-choice-neither" }),
        },
        Refused {
            case: "both members, each valid on its own".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &one, "both-assertion", "ATLEAST", 5000),
                "GatingRule": gating_member(panel_arn, &one, &one, "both-gating", "AND", 5000)
            }),
        },
        Refused {
            case: "both members under one and the same name".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &two, "both-same-name", "ATLEAST", 5000),
                "GatingRule": gating_member(panel_arn, &one, &two, "both-same-name", "OR", 5000)
            }),
        },
        Refused {
            case: "both members, the gating one over an unstored routing control".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &one, "both-unstored-control", "ATLEAST", 5000),
                "GatingRule": gating_member(panel_arn, std::slice::from_ref(&unstored_control), &one, "both-unstored-gating", "AND", 5000)
            }),
        },
        Refused {
            case: "both members, both scoped to an unstored control panel".to_string(),
            body: json!({
                "AssertionRule": assertion_member(&absent, &one, "both-absent-panel", "ATLEAST", 5000),
                "GatingRule": gating_member(&absent, &one, &one, "both-absent-gating", "AND", 5000)
            }),
        },
        Refused {
            case: "both members, with a ClientToken and Tags".to_string(),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &two, "both-token-assertion", "OR", 60000),
                "GatingRule": gating_member(panel_arn, &two, &one, "both-token-gating", "ATLEAST", 1),
                "ClientToken": "rule-choice-both",
                "Tags": { "suite": "rule-choice" }
            }),
        },
    ];

    // The choice is wrong whatever rule configuration the two members carry, so every
    // declared rule type appears on a both-members row too.
    for rule_type in RULE_TYPES {
        rows.push(Refused {
            case: format!("both members, both of rule type {rule_type}"),
            body: json!({
                "AssertionRule": assertion_member(panel_arn, &one, &format!("both-{rule_type}-assertion"), rule_type, 5000),
                "GatingRule": gating_member(panel_arn, &one, &two, &format!("both-{rule_type}-gating"), rule_type, 5000)
            }),
        });
    }
    rows
}

/// RULE: the two rule members of a create are the alternatives of one choice, so a request
/// carrying both or neither is refused `ValidationException` (400) with one fixed sentence,
/// and the refusal persists nothing.
#[tokio::test]
async fn a_create_carrying_both_rule_members_or_neither_is_refused_and_persists_nothing() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "rule-choice", 2).await;

    // A control panel with no rule yet, and the same panel once it holds one: a refusal
    // must neither create a rule nor disturb the rule that is already there.
    for phase in [
        "an empty control panel",
        "a control panel already holding one rule",
    ] {
        if phase == "a control panel already holding one rule" {
            let (status, answer) = create_rule(
                &router,
                json!({
                    "AssertionRule": assertion_member(
                        &panel_arn, &controls[..1], "rule-choice-baseline", "ATLEAST", 5000,
                    )
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case the baseline rule: expected 200, got {status} {answer:?}"
            );
        }

        let before = listed(&router, &panel_arn).await;
        let expected_before = if phase == "an empty control panel" {
            Vec::new()
        } else {
            vec![(
                "ASSERTION".to_string(),
                "rule-choice-baseline".to_string(),
            )]
        };
        assert_eq!(
            before, expected_before,
            "case {phase}: expected {expected_before:?} before any refusal, got {before:?}"
        );

        for row in refused_rows(&panel_arn, &controls) {
            let (status, answer) = create_rule(&router, row.body.clone()).await;
            assert_eq!(
                status, 400,
                "case {phase} / {}: expected 400, got {status} {answer:?}",
                row.case
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ValidationException",
                "case {phase} / {}: expected ValidationException, got {code:?} in {answer:?}",
                row.case
            );
            let actual = message(&answer);
            assert_eq!(
                actual, INVALID_RULE,
                "case {phase} / {}: expected {INVALID_RULE:?}, got {actual:?}",
                row.case
            );

            let after = listed(&router, &panel_arn).await;
            assert_eq!(
                after, before,
                "case {phase} / {}: a refused create persists nothing, expected {before:?}, got \
                 {after:?}",
                row.case
            );
        }
    }
}

/// RULE: a create carrying exactly one rule member succeeds, answers under that member
/// alone, and the rule it created is listed under its own kind key with the submitted name.
#[tokio::test]
async fn a_create_carrying_exactly_one_rule_member_is_served_and_listed_under_its_kind() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "one-member", 3).await;

    // Rows: both kinds, every declared rule type, and control lists of one to three
    // entries, so the served choice is not a property of one request shape.
    let mut rows: Vec<(String, &str, Value)> = Vec::new();
    for (index, rule_type) in RULE_TYPES.iter().enumerate() {
        let evaluated = controls[..=index].to_vec();
        let assertion_name = format!("one-assertion-{rule_type}");
        rows.push((
            assertion_name.clone(),
            "ASSERTION",
            json!({
                "AssertionRule": assertion_member(
                    &panel_arn, &evaluated, &assertion_name, rule_type, 5000 + index as i32,
                )
            }),
        ));
        let gating_name = format!("one-gating-{rule_type}");
        rows.push((
            gating_name.clone(),
            "GATING",
            json!({
                "GatingRule": gating_member(
                    &panel_arn,
                    &evaluated,
                    &controls[..1],
                    &gating_name,
                    rule_type,
                    5000 + index as i32,
                )
            }),
        ));
    }

    let mut expected: Vec<(String, String)> = Vec::new();
    for (name, kind, body) in &rows {
        let (status, answer) = create_rule(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {name}: expected 200, got {status} {answer:?}"
        );

        // The response fills the member of the kind created and leaves the other empty.
        let (filled, empty) = if *kind == "ASSERTION" {
            ("AssertionRule", "GatingRule")
        } else {
            ("GatingRule", "AssertionRule")
        };
        let arn = answer[filled]["SafetyRuleArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("{panel_arn}/safetyrule/")),
            "case {name}: expected a safety-rule ARN under {panel_arn}, got {arn:?}"
        );
        let served_name = answer[filled]["Name"].as_str().unwrap_or_default();
        assert_eq!(
            served_name, name,
            "case {name}: expected the submitted name, got {served_name:?}"
        );
        assert!(
            answer[empty].is_null(),
            "case {name}: expected no {empty} in the answer, got {:?}",
            answer[empty]
        );

        // And the control panel's list gains exactly that one rule, under its kind key.
        expected.push((kind.to_string(), name.clone()));
        let after = listed(&router, &panel_arn).await;
        assert_eq!(
            after, expected,
            "case {name}: expected {expected:?} listed, got {after:?}"
        );
    }
}

/// RULE: a refused create consumes nothing — neither the `ClientToken` it carried nor the
/// name it asked for, so the very same request minus the wrong member is served.
#[tokio::test]
async fn a_refused_create_consumes_neither_its_client_token_nor_its_name() {
    let router = router();
    let (panel_arn, controls) = panel_with_controls(&router, "consumes", 2).await;
    let one = controls[..1].to_vec();

    // Rows: the member the retry keeps, and the token the refused request carried.
    for (case, token, kind, retry) in [
        (
            "an assertion rule after a both-members refusal",
            "consumes-token-assertion",
            "ASSERTION",
            json!({
                "AssertionRule": assertion_member(&panel_arn, &one, "consumes-assertion", "ATLEAST", 5000)
            }),
        ),
        (
            "a gating rule after a both-members refusal",
            "consumes-token-gating",
            "GATING",
            json!({
                "GatingRule": gating_member(&panel_arn, &one, &one, "consumes-gating", "AND", 5000)
            }),
        ),
    ] {
        let name = retry[if kind == "ASSERTION" {
            "AssertionRule"
        } else {
            "GatingRule"
        }]["Name"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The refused request carries the token AND the name the retry will ask for.
        let mut both = retry.clone();
        let other = if kind == "ASSERTION" {
            json!(gating_member(&panel_arn, &one, &one, &name, "OR", 5000))
        } else {
            json!(assertion_member(&panel_arn, &one, &name, "OR", 5000))
        };
        both[if kind == "ASSERTION" {
            "GatingRule"
        } else {
            "AssertionRule"
        }] = other;
        both["ClientToken"] = json!(token);

        let (status, answer) = create_rule(&router, both).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual, INVALID_RULE,
            "case {case}: expected {INVALID_RULE:?}, got {actual:?}"
        );

        // Same token, same name, one member: served, because the refusal took neither.
        let mut served = retry.clone();
        served["ClientToken"] = json!(token);
        let (status, answer) = create_rule(&router, served).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the retry to be served 200, got {status} {answer:?}"
        );

        let listed_now = listed(&router, &panel_arn).await;
        assert!(
            listed_now.contains(&(kind.to_string(), name.clone())),
            "case {case}: expected ({kind}, {name}) listed, got {listed_now:?}"
        );
    }
}
