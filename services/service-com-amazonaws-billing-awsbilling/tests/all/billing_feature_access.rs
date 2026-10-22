// SPDX-License-Identifier: CC0-1.0

//! The per-feature access gate on the billing-preference operations, end to end
//! over the awsJson1_0 wire.
//!
//! One rule, asserted over every `BillingFeature` the model declares: the
//! preferences of a feature are reachable only while the account has access to
//! THAT feature (`ComAmazonawsBillingAwsbillingState::feature_access`). A request
//! naming a feature the account has no access to is refused 403
//! `AccessDeniedException` — on the read and on the write alike, since the
//! decision is per feature and not per operation — before anything is read or
//! written, so a refused read is distinguishable from an empty one and a refused
//! write stores nothing.
//!
//! The rows span what the rule generalises over: which feature (all seven,
//! including the two read-only history features), whether the account has access
//! to it, and which of the two operations asks — plus the accessible-feature
//! control, so the gate is never a blanket refusal.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::ACCESS_ACTIVATION_FEATURES;
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::billing_views::{call, error_code, standalone_emulator_with_engine, ACCOUNT_ID};
use crate::rejection_fidelity::message;

/// Every `BillingFeature` the model declares. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.billing#BillingFeature")`.
const ALL_FEATURES: &[&str] = &[
    "BILLING_ALERTS",
    "CREDIT_LEVEL_SHARING",
    "CREDIT_PREFERENCE_OPTIONS",
    "CREDIT_SHARING",
    "CREDIT_SHARING_HISTORY",
    "RI_SHARING",
    "RI_SHARING_HISTORY",
];

/// The two features the model marks read-only: "The history features
/// (RI_SHARING_HISTORY and CREDIT_SHARING_HISTORY) are read-only and cannot be
/// updated" (`UpdateBillingPreferences$feature` doc). An update of one is
/// refused 400 `ValidationException` — but only once the access gate has let the
/// request through.
const HISTORY_FEATURES: &[&str] = &["CREDIT_SHARING_HISTORY", "RI_SHARING_HISTORY"];

/// A preference key that is legal for every feature under test except
/// `CREDIT_PREFERENCE_OPTIONS`, whose keys must name one credit.
const PLAIN_KEY: &str = "sharing";

/// The key `CREDIT_PREFERENCE_OPTIONS` takes, in the documented
/// `credit/{creditId}/{option}` form.
const CREDIT_KEY: &str = "credit/CR-ACCESS-1/status";

/// The key an update of `feature` carries.
fn key_for(feature: &str) -> &'static str {
    if feature == "CREDIT_PREFERENCE_OPTIONS" {
        CREDIT_KEY
    } else {
        PLAIN_KEY
    }
}

/// `GetBillingPreferences` for one feature.
fn get_body(feature: &str) -> Value {
    json!({ "features": [feature] })
}

/// `UpdateBillingPreferences` for one feature, setting that feature's key.
fn update_body(feature: &str) -> Value {
    json!({
        "feature": feature,
        "billingPreferencesPerKey": [{ "key": key_for(feature), "value": "ENABLED" }],
    })
}

/// Grant or withdraw one feature's access on the engine the router serves from.
fn set_access(engine: &Arc<engine::InMemoryEngine>, feature: &str, granted: bool) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    if granted {
        state.grant_feature_access(feature);
    } else {
        state.revoke_feature_access(feature);
    }
}

/// How many preference keys the store holds.
fn stored_keys(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsBillingAwsbillingState>()
        .lock()
        .billing_preferences
        .len()
}

/// Assert one call is the modeled 403 `AccessDeniedException`, whose message is
/// built from the account, the operation and the feature the decision was made
/// from.
async fn assert_denied(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    feature: &str,
) {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: expected 403 AccessDeniedException, got {status} {answer:?}"
    );
    let stated = message(&answer);
    for expected in [feature, ACCOUNT_ID, operation] {
        assert!(
            stated.contains(expected),
            "case {case:?}: the refusal is built from {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_feature_the_account_cannot_reach_is_refused_on_both_operations() {
    // Every feature, both operations, with and without access: the gate is the
    // same decision for a read and for a write, and it is the ONLY thing that
    // changes between the two halves of each row.
    for feature in ALL_FEATURES {
        let (router, engine) = standalone_emulator_with_engine();

        set_access(&engine, feature, false);
        let case = format!("{feature} without access");
        assert_denied(&router, &case, "GetBillingPreferences", &get_body(feature), feature).await;
        assert_denied(
            &router,
            &case,
            "UpdateBillingPreferences",
            &update_body(feature),
            feature,
        )
        .await;
        assert_eq!(
            stored_keys(&engine),
            0,
            "case {case:?}: a refused write stores nothing"
        );

        // The same two requests once the account has access to that feature: the
        // read answers, and the write lands unless the model makes the feature
        // read-only.
        set_access(&engine, feature, true);
        let case = format!("{feature} with access");
        let (status, read) = call(&router, "GetBillingPreferences", &get_body(feature)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer, got {status} {read:?}"
        );
        let (status, written) =
            call(&router, "UpdateBillingPreferences", &update_body(feature)).await;
        let (expected_status, expected_code) = if HISTORY_FEATURES.contains(feature) {
            (400, "ValidationException")
        } else {
            (200, "")
        };
        assert_eq!(
            (status, error_code(&written).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status}, got {status} {written:?}"
        );
        assert_eq!(
            stored_keys(&engine),
            usize::from(expected_status == 200),
            "case {case:?}: the store holds exactly what the accepted write set"
        );
    }
}

#[tokio::test]
async fn a_refused_read_is_distinguishable_from_an_empty_one() {
    // The point of the gate: "no access to this feature" and "this feature has
    // no preferences" are different answers. Every row is a feature whose
    // preferences the account CAN reach while they are empty, then cannot.
    for feature in ALL_FEATURES {
        let (router, engine) = standalone_emulator_with_engine();
        set_access(&engine, feature, true);

        let (status, empty) = call(&router, "GetBillingPreferences", &get_body(feature)).await;
        assert_eq!(
            (status, empty["billingPreferences"].as_array().map(Vec::len)),
            (200, Some(0)),
            "case {feature:?}: an accessible feature with nothing set reports an empty list, \
             got {status} {empty:?}"
        );

        set_access(&engine, feature, false);
        assert_denied(
            &router,
            feature,
            "GetBillingPreferences",
            &get_body(feature),
            feature,
        )
        .await;
    }
}

#[tokio::test]
async fn the_gate_is_per_feature_so_one_feature_s_refusal_is_not_another_s() {
    // A caller that cannot reach one feature still reaches the others: the
    // decision is keyed by feature, so refusing every feature would fail this,
    // and so would refusing none.
    let (router, engine) = standalone_emulator_with_engine();
    for gated in ALL_FEATURES {
        for feature in ALL_FEATURES {
            set_access(&engine, feature, feature != gated);
        }
        for feature in ALL_FEATURES {
            let case = format!("{feature} while {gated} is gated");
            let body = get_body(feature);
            if feature == gated {
                assert_denied(&router, &case, "GetBillingPreferences", &body, feature).await;
            } else {
                let (status, answer) = call(&router, "GetBillingPreferences", &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: an accessible feature still answers, got {status} {answer:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_default_account_reaches_every_feature_except_the_activated_ones() {
    // The seeded account: the features behind the Billing-console access
    // activation are refused, every other one answers. This is the state the
    // emulator stands up with, so no test here sets anything first.
    let (router, _engine) = standalone_emulator_with_engine();

    for feature in ALL_FEATURES {
        let gated = ACCESS_ACTIVATION_FEATURES.contains(feature);
        let case = format!("default state, {feature}");
        if gated {
            assert_denied(
                &router,
                &case,
                "GetBillingPreferences",
                &get_body(feature),
                feature,
            )
            .await;
        } else {
            let (status, answer) = call(&router, "GetBillingPreferences", &get_body(feature)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a feature outside the activation answers, got {status} {answer:?}"
            );
        }
    }
}
