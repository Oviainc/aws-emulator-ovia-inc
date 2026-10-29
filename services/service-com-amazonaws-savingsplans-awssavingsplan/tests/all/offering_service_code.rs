// SPDX-License-Identifier: CC0-1.0

//! The service code an offering reports is one the offerings search accepts
//! back.
//!
//! `SavingsPlanOffering$serviceCode` and `DescribeSavingsPlansOfferings`'s own
//! `serviceCodes` filter target ONE shape — `SavingsPlanServiceCode`, letters
//! only, length 0..255 (source:
//! `smithy_query(query="shape",shape="SavingsPlanServiceCode")`, `@pattern
//! ^[a-zA-Z]+$`, `@length {min: 0, max: 255}`) — so the rule asserted here, over
//! the WHOLE seeded catalog rather than over one plan type:
//!
//! * every code the catalog reports satisfies that shape, so the filter admits
//!   it, and a search by it addresses exactly the offerings reporting it;
//! * a value the shape excludes is refused with the modelled `400
//!   ValidationException`, which is why the catalog may report none — and the
//!   refusal is asserted from the inside too, at the published length bound.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::ops::types::SAVINGS_PLAN_TYPE_VALUES;

use crate::savings_plans_lifecycle::{text, Emulator};

/// The longest `SavingsPlanServiceCode` the model admits.
///
/// Source: `smithy_query(query="shape",shape="SavingsPlanServiceCode")` —
/// `@length {min: 0, max: 255}`.
const MAX_SERVICE_CODE: usize = 255;

/// Whether `code` satisfies `SavingsPlanServiceCode`: at least one letter, at
/// most [`MAX_SERVICE_CODE`] of them, and nothing but letters (`@pattern
/// ^[a-zA-Z]+$`).
fn is_service_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_SERVICE_CODE
        && code.bytes().all(|b| b.is_ascii_alphabetic())
}

/// The offering ids a search by `serviceCodes: [code]` answers with.
async fn found_by(emulator: &Emulator, code: &str) -> Vec<String> {
    emulator
        .search_offerings(&json!({ "serviceCodes": [code] }))
        .await
        .iter()
        .map(|offering| text(offering, "offeringId"))
        .collect()
}

/// Every catalog row reports a service code inside the model's shape, and the
/// `serviceCodes` filter finds exactly the rows reporting it.
///
/// The rows the round-trip is asserted over are every DISTINCT code the catalog
/// reports — one per billing line item, spanning every plan type the model
/// declares and both the region-scoped and the unscoped plan types — plus a
/// well-formed code no offering reports.
#[tokio::test]
async fn every_offering_service_code_round_trips_through_the_offerings_filter() {
    let emulator = Emulator::new();
    let catalog = emulator.all_offerings().await;
    assert!(
        catalog.len() > 20,
        "the seeded catalog must be a catalog: got {} offerings",
        catalog.len()
    );

    let mut ids_by_code: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut codes_by_plan_type: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for offering in &catalog {
        let id = text(offering, "offeringId");
        let code = text(offering, "serviceCode");
        let plan_type = text(offering, "planType");
        assert!(
            is_service_code(&code),
            "case {id:?} ({plan_type}): serviceCode {code:?} must satisfy the model's \
             constraint, which is also what the serviceCodes filter accepts"
        );
        ids_by_code.entry(code.clone()).or_default().push(id);
        let seen = codes_by_plan_type.entry(plan_type).or_default();
        if !seen.contains(&code) {
            seen.push(code);
        }
    }

    // Every plan type the model declares sells offerings, and each bills under
    // exactly one line item, so none of them is left reporting nothing a caller
    // can search by.
    for plan_type in SAVINGS_PLAN_TYPE_VALUES {
        let codes = codes_by_plan_type.get(*plan_type).cloned().unwrap_or_default();
        assert_eq!(
            codes.len(),
            1,
            "case planType {plan_type:?}: expected one service code for the plan type, got \
             {codes:?}"
        );
    }

    // The round trip: each reported code, fed back into the same operation,
    // answers with exactly the offerings reporting it, in catalog order.
    for (code, expected) in &ids_by_code {
        let found = found_by(&emulator, code).await;
        assert!(
            !expected.is_empty(),
            "case serviceCodes {code:?}: a reported code names at least one offering"
        );
        assert_eq!(
            &found, expected,
            "case serviceCodes {code:?}: expected the {} offerings reporting it, got {}",
            expected.len(),
            found.len()
        );
    }

    // And a code the shape admits that no offering reports narrows the search to
    // nothing rather than being refused: `AWSLambda` is a letters-only service
    // code of the RATE catalog's enum
    // (`smithy_query(query="shape",shape="SavingsPlanRateServiceCode")`), which
    // no OFFERING bills under.
    let absent = "AWSLambda";
    assert!(
        !ids_by_code.contains_key(absent),
        "this row needs a code the catalog does not report, got {:?}",
        ids_by_code.keys().collect::<Vec<&String>>()
    );
    let found = found_by(&emulator, absent).await;
    assert!(
        found.is_empty(),
        "case serviceCodes {absent:?}: no offering reports it, so the search must answer with \
         none, got {found:?}"
    );
}

/// A `serviceCodes` value the model's shape excludes is refused with the
/// modelled 400 — the refusal the catalog would walk into if it reported such a
/// code — and a value at the published length bound passes the boundary.
#[tokio::test]
async fn a_service_code_outside_the_models_shape_is_refused() {
    let emulator = Emulator::new();

    // The rows differ along WHY the value is outside the shape: a digit (the
    // spelling of a per-service pricing code, which the RATE search's enum
    // carries and this pattern-constrained shape cannot), punctuation, a space,
    // the empty string the pattern's `+` excludes, and one byte past the length
    // bound. The list positions differ too, so the refusal is the walk's and not
    // one member's.
    let over_long = "a".repeat(MAX_SERVICE_CODE + 1);
    let refused: [(&str, Value); 6] = [
        (
            "a service code carrying a digit",
            json!({ "serviceCodes": ["AmazonEC2"], "maxResults": 5 }),
        ),
        (
            "a digit-carrying code after a well-formed one",
            json!({ "serviceCodes": ["AmazonSageMaker", "AmazonEC2"], "maxResults": 5 }),
        ),
        (
            "punctuation",
            json!({ "serviceCodes": ["Amazon-EC"], "maxResults": 5 }),
        ),
        (
            "a space",
            json!({ "serviceCodes": ["Amazon EC"], "maxResults": 5 }),
        ),
        (
            "the empty string",
            json!({ "serviceCodes": [""], "maxResults": 5 }),
        ),
        (
            "one byte past the length bound",
            json!({ "serviceCodes": [over_long.clone()], "maxResults": 5 }),
        ),
    ];

    for (case, request) in refused {
        let answer = emulator
            .call("DescribeSavingsPlansOfferings", request.clone())
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case {case:?} ({request}): expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // From the inside: a letters-only code AT the length bound is admitted, so
    // the rows above are refused for the shape and not for being unknown codes.
    for (case, code) in [
        ("one letter, the shortest the pattern admits", "a".to_string()),
        ("exactly the length bound", "a".repeat(MAX_SERVICE_CODE)),
    ] {
        let answer = emulator
            .call(
                "DescribeSavingsPlansOfferings",
                json!({ "serviceCodes": [code.clone()], "maxResults": 5 }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({} bytes): expected 200, got {} {:?}",
            code.len(),
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.list("searchResults"),
            &[] as &[Value],
            "case {case:?}: no offering reports it, so the answer must be empty, got {:?}",
            answer.body
        );
    }
}
