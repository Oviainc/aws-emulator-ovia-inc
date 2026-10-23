// SPDX-License-Identifier: CC0-1.0

//! `UpdatePreferences` writes, driven end-to-end over the wire (`awsJson1_0`)
//! through the same multi-service router the emulator boots.
//!
//! The rule under test: `UpdatePreferences`' three members
//! (`savingsEstimationMode`, `memberAccountDiscountVisibility`,
//! `preferredCommitment`) are independent, optional preferences with no
//! precondition on each other and none on the caller
//! (`smithy_query(query="operation", operation="UpdatePreferences")` models the
//! input as three optional members and declares no such rule), so ANY request
//! whose values are inside their modelled enums is accepted: the members it
//! supplies are merged into the account's stored record, it answers 200 with the
//! effective record, and a following `GetPreferences` reports the same record.
//! Only a value the model itself refuses is a 400, and that refusal is the
//! constraint walk's (`validation_exception_fields`).
//!
//! Every value in the tables below comes from the model's own enums
//! (`shared_types::{SAVINGS_ESTIMATION_MODE_VALUES,
//! MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, TERM_VALUES,
//! PAYMENT_OPTION_VALUES}`), so the rows span the whole class rather than the
//! one combination a recording happened to carry.

use serde_json::{json, Map, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;

use cost_optimization_hub::ops::types::shared_types::{
    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, PAYMENT_OPTION_VALUES,
    SAVINGS_ESTIMATION_MODE_VALUES, TERM_VALUES,
};

/// The `savingsEstimationMode` a read reports for an account that has set none —
/// read from the service rather than copied here, so the expectation cannot
/// drift from the mode the service applies.
use cost_optimization_hub::state::DEFAULT_SAVINGS_ESTIMATION_MODE;

use crate::cost_optimization_hub::{call, error_code, ok, router};

/// The defaults the service applies for the commitment members an account has
/// never set, from the model's own docs: `PreferredCommitment$term` documents
/// "If the value is null, it will default to `ThreeYears`" and
/// `$paymentOption` the same for `AllUpfront`.
const DEFAULT_TERM: &str = "ThreeYears";
const DEFAULT_PAYMENT_OPTION: &str = "AllUpfront";

/// The effective record that follows from a request's members, given what was
/// stored before it: every member the request states replaces the stored one,
/// every member it omits keeps its stored value, and a commitment member never
/// set reports the service's default. `memberAccountDiscountVisibility` is
/// reported only once it has been set, because the model documents no default
/// for it.
fn expected_record(stored: &Value, request: &Value) -> Value {
    let pick = |member: &str| -> Option<String> {
        request[member]
            .as_str()
            .or_else(|| stored[member].as_str())
            .map(str::to_string)
    };
    let commitment = |member: &str, default: &str| -> String {
        request["preferredCommitment"][member]
            .as_str()
            .or_else(|| stored["preferredCommitment"][member].as_str())
            .unwrap_or(default)
            .to_string()
    };

    let mut record = Map::new();
    record.insert(
        "savingsEstimationMode".to_string(),
        Value::String(
            pick("savingsEstimationMode")
                .unwrap_or_else(|| DEFAULT_SAVINGS_ESTIMATION_MODE.to_string()),
        ),
    );
    if let Some(visibility) = pick("memberAccountDiscountVisibility") {
        record.insert(
            "memberAccountDiscountVisibility".to_string(),
            Value::String(visibility),
        );
    }
    record.insert(
        "preferredCommitment".to_string(),
        json!({
            "term": commitment("term", DEFAULT_TERM),
            "paymentOption": commitment("paymentOption", DEFAULT_PAYMENT_OPTION),
        }),
    );
    Value::Object(record)
}

/// Every combination of the modelled enums is accepted on a fresh account: the
/// write answers 200 with exactly the record it stored, and a read reports the
/// same thing.
///
/// This is the rule at its widest — the full cross product of
/// `savingsEstimationMode` × `memberAccountDiscountVisibility` ×
/// `preferredCommitment.term` × `preferredCommitment.paymentOption` — so no
/// combination of values can be refused and none can be dropped on the way to
/// the store.
#[tokio::test]
async fn every_combination_of_the_modelled_preference_values_is_stored() {
    for mode in SAVINGS_ESTIMATION_MODE_VALUES {
        for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
            for term in TERM_VALUES {
                for payment_option in PAYMENT_OPTION_VALUES {
                    let case =
                        format!("{mode}/{visibility}/{term}/{payment_option}");
                    let request = json!({
                        "savingsEstimationMode": mode,
                        "memberAccountDiscountVisibility": visibility,
                        "preferredCommitment": {
                            "term": term,
                            "paymentOption": payment_option,
                        },
                    });
                    // A fresh account per row, so each row is judged on its own
                    // request rather than on what a previous row left behind.
                    let router = router();

                    let (status, updated) = call(&router, "UpdatePreferences", &request).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected HTTP 200, got {status} with {updated}"
                    );
                    let expected = expected_record(&Value::Null, &request);
                    assert_eq!(
                        updated, expected,
                        "case {case:?}: expected the effective record {expected}, got {updated}"
                    );

                    // The round trip, twice: the read reports the stored record
                    // and reading it does not change it.
                    for attempt in 0..2 {
                        let fetched = ok(&router, "GetPreferences", &json!({})).await;
                        assert_eq!(
                            fetched, expected,
                            "case {case:?} read {attempt}: expected {expected}, got {fetched}"
                        );
                    }
                }
            }
        }
    }
}

/// A sequence of writes merges: every member a request states replaces the
/// stored one and every member it omits keeps its stored value, whatever subset
/// of the three members the request carries — and the record can be written
/// back to an earlier value, which is what a client's teardown does.
#[tokio::test]
async fn successive_writes_merge_and_can_be_restored() {
    let router = router();

    // The requests, in order, spanning the subsets a client sends: one member
    // alone, a nested member alone, the organization-scoped member alone, two
    // at once, the empty request, and finally a restore of an earlier state.
    let requests: Vec<Value> = vec![
        json!({"savingsEstimationMode": "BeforeDiscounts"}),
        json!({"preferredCommitment": {"term": "OneYear"}}),
        json!({"preferredCommitment": {"paymentOption": "PartialUpfront"}}),
        json!({"memberAccountDiscountVisibility": "All"}),
        json!({
            "savingsEstimationMode": "AfterDiscounts",
            "preferredCommitment": {"paymentOption": "PartialUpfront", "term": "OneYear"},
        }),
        json!({}),
        json!({"memberAccountDiscountVisibility": "None"}),
        // The restore: the same shape of request the configure step sent, with
        // the values the account started from.
        json!({
            "savingsEstimationMode": "BeforeDiscounts",
            "preferredCommitment": {"paymentOption": "AllUpfront", "term": "ThreeYears"},
        }),
    ];

    let mut stored = Value::Null;
    for (index, request) in requests.iter().enumerate() {
        let case = format!("write {index}: {request}");
        let expected = expected_record(&stored, request);

        let (status, updated) = call(&router, "UpdatePreferences", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {updated}"
        );
        assert_eq!(
            updated, expected,
            "case {case:?}: expected the effective record {expected}, got {updated}"
        );

        let fetched = ok(&router, "GetPreferences", &json!({})).await;
        assert_eq!(
            fetched, expected,
            "case {case:?}: expected the read to report {expected}, got {fetched}"
        );

        // Writing the SAME request again is idempotent: the record the second
        // call answers is the record the first one left.
        let repeated = ok(&router, "UpdatePreferences", request).await;
        assert_eq!(
            repeated, expected,
            "case {case:?} repeated: expected {expected}, got {repeated}"
        );

        stored = expected;
    }
}

/// The only `UpdatePreferences` refusal is the model's own: a value outside a
/// member's enum is a 400 `ValidationException`, and it leaves the stored record
/// untouched — while the same member with a modelled value is accepted.
#[tokio::test]
async fn only_a_value_outside_the_model_is_refused() {
    let router = router();

    let accepted = json!({
        "savingsEstimationMode": "BeforeDiscounts",
        "memberAccountDiscountVisibility": "All",
        "preferredCommitment": {"term": "OneYear", "paymentOption": "NoUpfront"},
    });
    let before = ok(&router, "UpdatePreferences", &accepted).await;

    // (case, request) — one row per member that carries an enum, plus a nested
    // one and two at once.
    let cases: Vec<(&str, Value)> = vec![
        (
            "savingsEstimationMode outside its enum",
            json!({"savingsEstimationMode": "Afterwards"}),
        ),
        (
            "memberAccountDiscountVisibility outside its enum",
            json!({"memberAccountDiscountVisibility": "Everyone"}),
        ),
        (
            "preferredCommitment.term outside its enum",
            json!({"preferredCommitment": {"term": "TwoYears"}}),
        ),
        (
            "preferredCommitment.paymentOption outside its enum",
            json!({"preferredCommitment": {"paymentOption": "SomeUpfront"}}),
        ),
        (
            "two members outside their enums",
            json!({
                "savingsEstimationMode": "Afterwards",
                "preferredCommitment": {"term": "TwoYears"},
            }),
        ),
    ];

    for (case, request) in &cases {
        let (status, payload) = call(&router, "UpdatePreferences", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {payload}"
        );

        let fetched = ok(&router, "GetPreferences", &json!({})).await;
        assert_eq!(
            fetched, before,
            "case {case:?}: a refused write must leave the record at {before}, got {fetched}"
        );
    }
}
