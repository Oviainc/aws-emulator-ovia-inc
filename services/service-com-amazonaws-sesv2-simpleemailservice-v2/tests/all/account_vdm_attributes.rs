// SPDX-License-Identifier: CC0-1.0

//! The account-level VDM attributes, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised.
//!
//! The rule these cases pin: `PutAccountVdmAttributes` REPLACES the account's VDM
//! attributes with the ones the request carries, so a later read of the account
//! reports exactly those — a member the request leaves out is cleared rather than
//! kept — and a value outside a member's modelled value set is refused with
//! `BadRequestException`, leaving the stored attributes as they were.
//!
//! The table spans what the rule generalises over: every `FeatureStatus` the VDM
//! status can be switched to, whether each sub-attribute object is carried at all,
//! carried empty or carried with each published status inside it, and every
//! combination of the two sub-attributes.

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::FEATURE_STATUS_VALUES;

use crate::configuration_sets::{Reply, Ses};

/// `PutAccountVdmAttributes`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="PutAccountVdmAttributes")`:
/// `PUT /v2/email/account/vdm`).
const VDM_PATH: &str = "/v2/email/account/vdm";

/// `GetAccount`'s route, which reads the stored attributes back.
const ACCOUNT_PATH: &str = "/v2/email/account";

async fn put_vdm(ses: &Ses, attributes: Value) -> Reply {
    ses.call("PUT", VDM_PATH, Some(json!({"VdmAttributes": attributes})))
        .await
}

/// The `VdmAttributes` the account reports, or `Value::Null` when it reports none.
async fn reported_attributes(ses: &Ses) -> Value {
    ses.call("GET", ACCOUNT_PATH, None)
        .await
        .member("VdmAttributes")
}

/// One row of the replace-and-read table: the `VdmAttributes` the request carries,
/// and the ones a read of the account must report afterwards.
struct Case {
    case: String,
    sent: Value,
    expected: Value,
}

/// The rows, built from the model's own `FeatureStatus` value set so every published
/// value is covered rather than only the ones a caller happened to use.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();

    for status in FEATURE_STATUS_VALUES {
        // The status on its own: both sub-attributes are left out, so neither is
        // reported.
        cases.push(Case {
            case: format!("{status}, no sub-attributes"),
            sent: json!({"VdmEnabled": status}),
            expected: json!({"VdmEnabled": status}),
        });

        // Each sub-attribute object carried but empty states no preference inside
        // it, so the account reports the object with nothing in it.
        cases.push(Case {
            case: format!("{status}, empty dashboard attributes"),
            sent: json!({"VdmEnabled": status, "DashboardAttributes": {}}),
            expected: json!({"VdmEnabled": status, "DashboardAttributes": {}}),
        });
        cases.push(Case {
            case: format!("{status}, empty guardian attributes"),
            sent: json!({"VdmEnabled": status, "GuardianAttributes": {}}),
            expected: json!({"VdmEnabled": status, "GuardianAttributes": {}}),
        });

        // One row per status each sub-attribute can hold, alone and then both
        // together, which is every combination of the three members.
        for metrics in FEATURE_STATUS_VALUES {
            cases.push(Case {
                case: format!("{status}, engagement metrics {metrics}"),
                sent: json!({
                    "VdmEnabled": status,
                    "DashboardAttributes": {"EngagementMetrics": metrics},
                }),
                expected: json!({
                    "VdmEnabled": status,
                    "DashboardAttributes": {"EngagementMetrics": metrics},
                }),
            });
            for delivery in FEATURE_STATUS_VALUES {
                cases.push(Case {
                    case: format!(
                        "{status}, engagement metrics {metrics}, optimized shared delivery {delivery}"
                    ),
                    sent: json!({
                        "VdmEnabled": status,
                        "DashboardAttributes": {"EngagementMetrics": metrics},
                        "GuardianAttributes": {"OptimizedSharedDelivery": delivery},
                    }),
                    expected: json!({
                        "VdmEnabled": status,
                        "DashboardAttributes": {"EngagementMetrics": metrics},
                        "GuardianAttributes": {"OptimizedSharedDelivery": delivery},
                    }),
                });
            }
        }
        for delivery in FEATURE_STATUS_VALUES {
            cases.push(Case {
                case: format!("{status}, optimized shared delivery {delivery}"),
                sent: json!({
                    "VdmEnabled": status,
                    "GuardianAttributes": {"OptimizedSharedDelivery": delivery},
                }),
                expected: json!({
                    "VdmEnabled": status,
                    "GuardianAttributes": {"OptimizedSharedDelivery": delivery},
                }),
            });
        }
    }

    cases
}

#[tokio::test]
async fn put_account_vdm_attributes_persists_what_the_request_carries() {
    for case in cases() {
        let case_name = case.case.clone();
        // A fresh service per row, so each row is answered from the attributes it
        // wrote and nothing else.
        let ses = Ses::new();

        let put = put_vdm(&ses, case.sent.clone()).await;
        assert_eq!(
            put.status,
            200,
            "case {case_name:?}: expected 200 from the VDM put, got {} {:?}",
            put.status,
            put.message()
        );
        assert!(
            put.is_empty_body(),
            "case {case_name:?}: PutAccountVdmAttributesResponse models no members, so the body must be empty, got {:?}",
            put.member("VdmAttributes")
        );

        let reported = reported_attributes(&ses).await;
        assert_eq!(
            reported, case.expected,
            "case {case_name:?}: expected the account to report {:?}, got {reported:?}",
            case.expected
        );

        // Reading the account does not change what a later caller sees.
        let again = reported_attributes(&ses).await;
        assert_eq!(
            again, case.expected,
            "case {case_name:?}: expected a second read to report {:?}, got {again:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn each_put_replaces_the_previous_attributes() {
    let ses = Ses::new();

    // What the account holds before anything configures it: VDM is off, and neither
    // sub-attribute holds a preference.
    let initial = reported_attributes(&ses).await;
    assert_eq!(
        initial,
        json!({"VdmEnabled": "DISABLED"}),
        "case \"initial\": expected VDM off with no sub-attributes, got {initial:?}"
    );

    // Each step replaces the whole sub-resource, so the attributes after it are the
    // ones that step carried — never the union with an earlier step's.
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "enable VDM with both sub-attributes on",
            json!({
                "VdmEnabled": "ENABLED",
                "DashboardAttributes": {"EngagementMetrics": "ENABLED"},
                "GuardianAttributes": {"OptimizedSharedDelivery": "ENABLED"},
            }),
            json!({
                "VdmEnabled": "ENABLED",
                "DashboardAttributes": {"EngagementMetrics": "ENABLED"},
                "GuardianAttributes": {"OptimizedSharedDelivery": "ENABLED"},
            }),
        ),
        (
            "keep VDM on but drop the guardian attributes",
            json!({
                "VdmEnabled": "ENABLED",
                "DashboardAttributes": {"EngagementMetrics": "DISABLED"},
            }),
            json!({
                "VdmEnabled": "ENABLED",
                "DashboardAttributes": {"EngagementMetrics": "DISABLED"},
            }),
        ),
        (
            "guardian only, dropping the dashboard attributes",
            json!({
                "VdmEnabled": "ENABLED",
                "GuardianAttributes": {"OptimizedSharedDelivery": "DISABLED"},
            }),
            json!({
                "VdmEnabled": "ENABLED",
                "GuardianAttributes": {"OptimizedSharedDelivery": "DISABLED"},
            }),
        ),
        (
            "turn VDM off, which clears both sub-attributes",
            json!({"VdmEnabled": "DISABLED"}),
            json!({"VdmEnabled": "DISABLED"}),
        ),
        (
            "turn VDM back on again",
            json!({"VdmEnabled": "ENABLED"}),
            json!({"VdmEnabled": "ENABLED"}),
        ),
    ];

    for (case, attributes, expected) in steps {
        let put = put_vdm(&ses, attributes).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected 200 from the VDM put, got {}",
            put.status
        );
        let reported = reported_attributes(&ses).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the account to report {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_its_modelled_set_is_refused_and_changes_nothing() {
    // Each row carries a value no member's value set holds, on each of the three
    // members in turn, plus values that differ from a published one only in case or
    // by surrounding space.
    let rows: Vec<(&str, Value)> = vec![
        ("unknown VDM status", json!({"VdmEnabled": "ON"})),
        ("lower-case VDM status", json!({"VdmEnabled": "enabled"})),
        ("empty VDM status", json!({"VdmEnabled": ""})),
        ("padded VDM status", json!({"VdmEnabled": " ENABLED"})),
        (
            "unknown engagement-metrics status",
            json!({
                "VdmEnabled": "ENABLED",
                "DashboardAttributes": {"EngagementMetrics": "OFF"},
            }),
        ),
        (
            "lower-case optimized-shared-delivery status",
            json!({
                "VdmEnabled": "ENABLED",
                "GuardianAttributes": {"OptimizedSharedDelivery": "disabled"},
            }),
        ),
        (
            "good status with one unknown sub-attribute status",
            json!({
                "VdmEnabled": "ENABLED",
                "DashboardAttributes": {"EngagementMetrics": "ENABLED"},
                "GuardianAttributes": {"OptimizedSharedDelivery": "ACTIVE"},
            }),
        ),
    ];

    for (case, attributes) in rows {
        let ses = Ses::new();
        // A known state to refuse against, so the assertion shows the refusal kept
        // it rather than that nothing had been configured.
        let settled = json!({
            "VdmEnabled": "ENABLED",
            "DashboardAttributes": {"EngagementMetrics": "ENABLED"},
        });
        let prepared = put_vdm(&ses, settled.clone()).await;
        assert_eq!(
            prepared.status, 200,
            "case {case:?}: expected 200 while preparing the account, got {}",
            prepared.status
        );

        let refused = put_vdm(&ses, attributes).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: expected the refusal to name what was invalid, got an empty message"
        );

        let reported = reported_attributes(&ses).await;
        assert_eq!(
            reported, settled,
            "case {case:?}: expected the refused put to leave {settled:?} in place, got {reported:?}"
        );
    }
}
