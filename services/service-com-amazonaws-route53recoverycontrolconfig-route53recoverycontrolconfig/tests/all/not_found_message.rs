// SPDX-License-Identifier: CC0-1.0

//! The sentence this service refuses an ARN that resolves to nothing with.
//!
//! Every request that resolves a recovery-control ARN and finds no such resource is
//! refused with `ResourceNotFoundException` (404) whose `message` is one fixed sentence:
//! `No record corresponding to <resource noun> Arn(<the request's ARN>)`. Only the noun
//! of the resource kind that was looked up and the ARN the request asked for vary; the
//! ARN is echoed back exactly as it was sent.
//!
//! The tables span what that rule generalises over: every implemented operation that
//! ADDRESSES a resource by ARN (read, delete, update, list), every resource level
//! (cluster, control panel, routing control, safety rule) and several fillings of the
//! ARN's own variable parts — a lowercase UUID, an upper-case-hex UUID and a non-UUID
//! tail.
//!
//! Every ARN here names the CALLER's account, because that is the only kind of ARN this
//! sentence answers: one naming another account is refused by the authorization check
//! ahead of the lookup, which `tests/all/cross_account_authorization.rs` pins.
//!
//! A create that merely REFERENCES a parent by ARN is NOT in this table: it resolves no
//! resource of its own, so it is refused on its input instead — see
//! `tests/all/rejection_error_classes.rs`.

use serde_json::{json, Value};

use super::common::{call, create_cluster, enc, error_code, router};

/// A recovery-control ARN: AWS writes them `arn:aws:route53-recovery-control::<account>:…`
/// with an empty Region segment.
fn arn(account: &str, tail: &str) -> String {
    format!("arn:aws:route53-recovery-control::{account}:{tail}")
}

/// The sentence AWS refuses an unresolvable ARN with, restated here independently of the
/// service's own code: fixed text, with the resource noun and the request's ARN as its
/// only variable parts.
fn expected(noun: &str, requested_arn: &str) -> String {
    format!("No record corresponding to {noun} Arn({requested_arn})")
}

/// The `Message` of an error body, under the `message` jsonName the model gives it
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Four fillings of a resource id: the all-zero UUID, a lowercase UUID, an
/// upper-case-hex UUID and a non-UUID token. All four satisfy the pattern the model puts
/// on the ARN members that carry one (`^[A-Za-z0-9:\/_-]*$`, via
/// `smithy_query(query="operation",operation="UpdateCluster")`).
const RESOURCE_IDS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4",
    "ABCDEF01-2345-6789-ABCD-EF0123456789",
    "named_cluster-1",
];

/// The account the ARNs are built for: the caller's own, which is the only account whose
/// ARNs reach a lookup at all.
const ACCOUNT: &str = "000000000000";

/// One request to make, with the resource kind and ARN its refusal must name.
struct Case {
    label: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    noun: &'static str,
    requested_arn: String,
}

/// Every cluster-resolving request for one cluster ARN.
fn cluster_cases(cluster_arn: &str) -> Vec<Case> {
    vec![
        Case {
            label: format!("DescribeCluster({cluster_arn})"),
            method: "GET",
            path: format!("/cluster/{}", enc(cluster_arn)),
            body: None,
            noun: "cluster",
            requested_arn: cluster_arn.to_string(),
        },
        Case {
            label: format!("DeleteCluster({cluster_arn})"),
            method: "DELETE",
            path: format!("/cluster/{}", enc(cluster_arn)),
            body: None,
            noun: "cluster",
            requested_arn: cluster_arn.to_string(),
        },
        Case {
            label: format!("UpdateCluster({cluster_arn})"),
            method: "PUT",
            path: "/cluster".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "NetworkType": "DUALSTACK" })),
            noun: "cluster",
            requested_arn: cluster_arn.to_string(),
        },
        Case {
            label: format!("ListControlPanels(ClusterArn={cluster_arn})"),
            method: "GET",
            path: format!("/controlpanels?ClusterArn={}", enc(cluster_arn)),
            body: None,
            noun: "cluster",
            requested_arn: cluster_arn.to_string(),
        },
    ]
}

/// Every control-panel-resolving request for one control panel ARN.
fn control_panel_cases(panel_arn: &str) -> Vec<Case> {
    vec![
        Case {
            label: format!("DescribeControlPanel({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}", enc(panel_arn)),
            body: None,
            noun: "control panel",
            requested_arn: panel_arn.to_string(),
        },
        Case {
            label: format!("DeleteControlPanel({panel_arn})"),
            method: "DELETE",
            path: format!("/controlpanel/{}", enc(panel_arn)),
            body: None,
            noun: "control panel",
            requested_arn: panel_arn.to_string(),
        },
        Case {
            label: format!("ListRoutingControls({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}/routingcontrols", enc(panel_arn)),
            body: None,
            noun: "control panel",
            requested_arn: panel_arn.to_string(),
        },
        Case {
            label: format!("ListSafetyRules({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}/safetyrules", enc(panel_arn)),
            body: None,
            noun: "control panel",
            requested_arn: panel_arn.to_string(),
        },
        Case {
            label: format!("UpdateControlPanel({panel_arn})"),
            method: "PUT",
            path: "/controlpanel".to_string(),
            body: Some(json!({ "ControlPanelArn": panel_arn, "ControlPanelName": "renamed" })),
            noun: "control panel",
            requested_arn: panel_arn.to_string(),
        },
    ]
}

/// Every safety-rule-resolving request for one safety rule ARN.
fn safety_rule_cases(rule_arn: &str) -> Vec<Case> {
    vec![
        Case {
            label: format!("DescribeSafetyRule({rule_arn})"),
            method: "GET",
            path: format!("/safetyrule/{}", enc(rule_arn)),
            body: None,
            noun: "safety rule",
            requested_arn: rule_arn.to_string(),
        },
        Case {
            label: format!("DeleteSafetyRule({rule_arn})"),
            method: "DELETE",
            path: format!("/safetyrule/{}", enc(rule_arn)),
            body: None,
            noun: "safety rule",
            requested_arn: rule_arn.to_string(),
        },
        Case {
            label: format!("UpdateSafetyRule({rule_arn})"),
            method: "PUT",
            path: "/safetyrule".to_string(),
            body: Some(json!({
                "AssertionRuleUpdate": {
                    "Name": "renamed",
                    "SafetyRuleArn": rule_arn,
                    "WaitPeriodMs": 5000
                }
            })),
            noun: "safety rule",
            requested_arn: rule_arn.to_string(),
        },
    ]
}

/// Every routing-control-resolving request for one routing control ARN.
fn routing_control_cases(control_arn: &str) -> Vec<Case> {
    vec![
        Case {
            label: format!("DescribeRoutingControl({control_arn})"),
            method: "GET",
            path: format!("/routingcontrol/{}", enc(control_arn)),
            body: None,
            noun: "routing control",
            requested_arn: control_arn.to_string(),
        },
        Case {
            label: format!("DeleteRoutingControl({control_arn})"),
            method: "DELETE",
            path: format!("/routingcontrol/{}", enc(control_arn)),
            body: None,
            noun: "routing control",
            requested_arn: control_arn.to_string(),
        },
    ]
}

/// Every operation that resolves an ARN answers the one fixed sentence, naming the kind
/// of resource it looked for and echoing the ARN the request sent.
#[tokio::test]
async fn an_unresolvable_arn_is_refused_with_the_no_record_sentence() {
    let router = router();

    // A real cluster exists throughout, so a refusal is about the ARN the request names
    // and not about an empty store.
    create_cluster(&router, json!({ "ClusterName": "present" })).await;

    let mut cases: Vec<Case> = Vec::new();
    for id in RESOURCE_IDS {
        let cluster_arn = arn(ACCOUNT, &format!("cluster/{id}"));
        let panel_arn = arn(ACCOUNT, &format!("controlpanel/{id}"));
        let control_arn = format!("{panel_arn}/routingcontrol/{id}");
        let rule_arn = format!("{panel_arn}/safetyrule/{id}");
        cases.extend(cluster_cases(&cluster_arn));
        cases.extend(control_panel_cases(&panel_arn));
        cases.extend(routing_control_cases(&control_arn));
        cases.extend(safety_rule_cases(&rule_arn));
    }

    for case in cases {
        let (status, answer) = call(&router, case.method, &case.path, case.body.clone()).await;
        assert_eq!(
            status, 404,
            "case {}: expected 404, got {status} {answer:?}",
            case.label
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {}: expected ResourceNotFoundException, got {answer:?}",
            case.label
        );
        let actual = message(&answer);
        assert_eq!(
            actual,
            expected(case.noun, &case.requested_arn),
            "case {}: expected the fixed not-found sentence, got {actual:?}",
            case.label
        );
    }
}

/// The ARN in the sentence is the one the request carried, so an ARN this service itself
/// minted and then deleted comes back inside `Arn(...)` unchanged — and a resource that
/// does exist is still answered, not refused.
#[tokio::test]
async fn the_sentence_echoes_a_minted_arn_and_a_live_resource_is_still_served() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "round-trip" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();

    // Positive control: the ARN resolves while the cluster is stored.
    let (status, read) = call(
        &router,
        "GET",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case live cluster: expected DescribeCluster 200, got {status} {read:?}"
    );
    assert_eq!(
        read["Cluster"]["ClusterArn"].as_str(),
        Some(cluster_arn.as_str()),
        "case live cluster: expected the requested ARN back, got {:?}",
        read["Cluster"]["ClusterArn"]
    );

    // Its default control panel goes with it, so the cluster is deletable on its own.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete the cluster: expected 200, got {status} {deleted:?}"
    );

    let (status, gone) = call(
        &router,
        "GET",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case deleted cluster: expected 404, got {status} {gone:?}"
    );
    let actual = message(&gone);
    assert_eq!(
        actual,
        expected("cluster", &cluster_arn),
        "case deleted cluster: expected the minted ARN echoed in the sentence, got {actual:?}"
    );
}
