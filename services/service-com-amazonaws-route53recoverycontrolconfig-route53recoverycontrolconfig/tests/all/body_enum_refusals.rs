// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request BODY carrying a value outside an enumerated
//! type's declared set with.
//!
//! The rule under test: such a value never reaches the operation — the request-body
//! validator in front of this service refuses it with `BadRequestException` (400), whose
//! message lists, in brackets, the clause that validator states for the enumerated class:
//! the value the caller submitted and every value the model declares for the type, in the
//! order it declares them.
//!
//! The table spans what that rule generalises over rather than the one request that
//! exposed it: both enumerated types this service's bodies carry (`NetworkType` and
//! `RuleType`), a member at the top level of the body and one nested two structures deep,
//! three operations and three HTTP methods, and four forms of a value that misses the set
//! — a value of no declared spelling, a declared value in the wrong case, a declared
//! value with other characters around it, and the empty string.
//!
//! Its mirror is `tests/all/rejection_error_classes.rs`, which pins the same layer's
//! clauses for the `@pattern` and `@length` classes and the classes it states no clause
//! for at all.

use serde_json::{json, Value};

use super::common::{call, create_cluster, error_code, router};

/// The values `NetworkType` declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="NetworkType")`).
pub(crate) const NETWORK_TYPE_VALUES: &[&str] = &["IPV4", "DUALSTACK"];

/// The values `RuleType` declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="RuleType")`).
const RULE_TYPE_VALUES: &[&str] = &["ATLEAST", "AND", "OR"];

/// The whole message the body validator refuses a value outside an enumerated set with,
/// restated here independently of the service's own code.
pub(crate) fn enum_clause(value: &str, permitted: &[&str]) -> String {
    let quoted: Vec<String> = permitted
        .iter()
        .map(|admitted| format!("\"{admitted}\""))
        .collect();
    format!(
        "Invalid request body : [instance value (\"{value}\") not found in enum (possible \
         values: [{}])]",
        quoted.join(",")
    )
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ValidationException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// A `CreateSafetyRule` body whose assertion rule is complete and valid except for the
/// `RuleConfig$Type` it carries, so the only constraint the request breaks is that
/// enumerated one (a missing `@required` member would be a class the door states no
/// clause for, which would leave the whole refusal to the modelled error instead).
fn safety_rule_body(control_panel_arn: &str, rule_type: &str) -> Value {
    json!({
        "AssertionRule": {
            "AssertedControls": [format!("{control_panel_arn}/routingcontrol/asserted")],
            "ControlPanelArn": control_panel_arn,
            "Name": "assertion",
            "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": rule_type },
            "WaitPeriodMs": 5000
        }
    })
}

/// One request to make, with the value its refusal must quote and the set it must list.
struct Case {
    label: &'static str,
    method: &'static str,
    path: &'static str,
    body: Value,
    value: &'static str,
    permitted: &'static [&'static str],
}

/// RULE: a body value outside its enumerated type's declared set is refused by the body
/// validator — `BadRequestException` (400) stating the submitted value and every declared
/// value — whichever operation, method and depth the member sits at.
#[tokio::test]
async fn a_body_value_outside_its_enumerated_set_is_refused_by_the_body_validator() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "enumerated" })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let panel_arn = format!(
        "arn:aws:route53-recovery-control::000000000000:controlpanel/{}",
        "00000000-0000-0000-0000-000000000000"
    );

    let cases: Vec<Case> = vec![
        Case {
            label: "a NetworkType of no declared spelling, on a create",
            method: "POST",
            path: "/cluster",
            body: json!({ "ClusterName": "enum-create", "NetworkType": "NOTAVALIDTYPE" }),
            value: "NOTAVALIDTYPE",
            permitted: NETWORK_TYPE_VALUES,
        },
        Case {
            label: "a declared NetworkType in the wrong case, on a create",
            method: "POST",
            path: "/cluster",
            body: json!({ "ClusterName": "enum-case", "NetworkType": "ipv4" }),
            value: "ipv4",
            permitted: NETWORK_TYPE_VALUES,
        },
        Case {
            label: "an empty NetworkType, on a create",
            method: "POST",
            path: "/cluster",
            body: json!({ "ClusterName": "enum-empty", "NetworkType": "" }),
            value: "",
            permitted: NETWORK_TYPE_VALUES,
        },
        Case {
            label: "a NetworkType of no declared spelling, on an update",
            method: "PUT",
            path: "/cluster",
            body: json!({ "ClusterArn": cluster_arn, "NetworkType": "IPV6" }),
            value: "IPV6",
            permitted: NETWORK_TYPE_VALUES,
        },
        Case {
            label: "a declared NetworkType with other characters around it, on an update",
            method: "PUT",
            path: "/cluster",
            body: json!({ "ClusterArn": cluster_arn, "NetworkType": "DUALSTACK-ONLY" }),
            value: "DUALSTACK-ONLY",
            permitted: NETWORK_TYPE_VALUES,
        },
        Case {
            // Two structures deep in the body, and the service's OTHER enumerated type:
            // the clause is read off the member's own type, so depth changes nothing.
            label: "a RuleType of no declared spelling, nested in a safety rule",
            method: "POST",
            path: "/safetyrule",
            body: safety_rule_body(&panel_arn, "MAYBE"),
            value: "MAYBE",
            permitted: RULE_TYPE_VALUES,
        },
        Case {
            label: "a declared RuleType in the wrong case, nested in a safety rule",
            method: "POST",
            path: "/safetyrule",
            body: safety_rule_body(&panel_arn, "atleast"),
            value: "atleast",
            permitted: RULE_TYPE_VALUES,
        },
    ];

    for case in &cases {
        let (status, answer) = call(&router, case.method, case.path, Some(case.body.clone())).await;
        assert_eq!(
            status, 400,
            "case {}: expected 400, got {status} {answer:?}",
            case.label
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {}: expected BadRequestException, got {answer:?}",
            case.label
        );
        let actual = message(&answer);
        let expected = enum_clause(case.value, case.permitted);
        assert_eq!(
            actual, expected,
            "case {}: expected {expected:?}, got {actual:?}",
            case.label
        );
    }
}

/// RULE, the other way round: a value the enumerated type DOES declare is not the body
/// validator's business, so the request passes the door and the operation behind it
/// answers.
#[tokio::test]
async fn a_declared_enumerated_value_passes_the_body_validator() {
    let router = router();

    let panel_arn = "arn:aws:route53-recovery-control::000000000000:controlpanel/declared";

    for network_type in NETWORK_TYPE_VALUES {
        let created = create_cluster(
            &router,
            json!({ "ClusterName": format!("declared-{network_type}"), "NetworkType": network_type }),
        )
        .await;
        assert_eq!(
            created["NetworkType"],
            json!(network_type),
            "case NetworkType {network_type}: expected it stored, got {:?}",
            created["NetworkType"]
        );
    }

    // The nested member too: whatever the operation behind the door answers (this one is
    // not implemented yet), it is not the door's body refusal.
    for rule_type in RULE_TYPE_VALUES {
        let (status, answer) = call(
            &router,
            "POST",
            "/safetyrule",
            Some(safety_rule_body(panel_arn, rule_type)),
        )
        .await;
        assert_ne!(
            error_code(&answer),
            "BadRequestException",
            "case RuleType {rule_type}: a declared value must pass the body validator, got \
             {status} {answer:?}"
        );
    }
}
