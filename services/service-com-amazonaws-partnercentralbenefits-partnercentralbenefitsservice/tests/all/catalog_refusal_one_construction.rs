// SPDX-License-Identifier: CC0-1.0

//! The single construction of the catalog-not-found refusal, driven end to end over the
//! wire through one operation per former hand-rolled copy.
//!
//! The rule under test: every operation that binds a required `Catalog` answers a request
//! naming a catalog this service does not hold with the ONE refusal
//! `ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::catalog_refusal`
//! builds, so the wire answer of every such operation moves with that helper and none of
//! them can carry its own wording.
//!
//! The table spans the class rather than one call: the three operations that used to build
//! the refusal inline (`CreateBenefitApplication`, `ListBenefits`,
//! `ListBenefitApplications`) alongside two that have always gone through the helper
//! (`ListBenefitAllocations`, `GetBenefit`), crossed with several catalog names the
//! modelled `CatalogName` pattern admits (`^[A-Za-z0-9_-]+$`,
//! `smithy_query(query="shape",shape="CatalogName")`, which bounds the length not at all),
//! and — on the other side of the rule — with every catalog the service does hold.
//!
//! Each expected message is taken from the helper itself, read out of the live store, so
//! a change to what the helper builds changes what every row must answer.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness};
use crate::service_error_rules::{message_of, UNKNOWN_BENEFIT};

/// The refusal the state helper builds for `catalog` in this harness's own store: the one
/// construction every row below must answer with, and `None` when the catalog exists.
fn helper_refusal(harness: &Harness, catalog: &str) -> Option<String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .catalog_refusal(catalog)
        .map(|refusal| refusal.message)
}

/// One well-formed request per operation in the table, all naming `catalog`.
///
/// The first three are the operations that used to rebuild the refusal by hand; the last
/// two have always called the helper, so they are the reference the first three must agree
/// with.
fn catalog_bound_requests(
    catalog: &str,
    benefit_identifier: &str,
    client_token: &str,
) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateBenefitApplication",
            json!({
                "Catalog": catalog,
                "ClientToken": client_token,
                "BenefitIdentifier": benefit_identifier,
                "Name": "a catalog-refusal probe",
            }),
        ),
        ("ListBenefits", json!({ "Catalog": catalog })),
        ("ListBenefitApplications", json!({ "Catalog": catalog })),
        ("ListBenefitAllocations", json!({ "Catalog": catalog })),
        (
            "GetBenefit",
            json!({ "Catalog": catalog, "Identifier": benefit_identifier }),
        ),
    ]
}

/// Catalog names this service answers to on no operation, each admitted by the modelled
/// `CatalogName` pattern: a hyphenated name, digits only, an underscored name, a single
/// character and a long one.
fn unknown_catalogs() -> Vec<(&'static str, String)> {
    vec![
        ("a hyphenated name", "no-such-catalog-19740317".to_string()),
        ("digits only", "4815162342".to_string()),
        ("an underscored name", "no_such_catalog".to_string()),
        ("a single character", "q".to_string()),
        ("a long name", "k".repeat(312)),
    ]
}

/// Every operation in the table answers a catalog this service does not hold with exactly
/// the refusal the state helper builds — the three former inline copies included.
#[tokio::test]
async fn every_operation_answers_the_refusal_the_one_helper_builds() {
    let harness = Harness::new();
    for (index, (case, catalog)) in unknown_catalogs().into_iter().enumerate() {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog.as_str()),
            "case {case:?}: {catalog:?} must name no catalog this service serves"
        );
        let expected = helper_refusal(&harness, &catalog).unwrap_or_else(|| {
            panic!("case {case:?}: the helper must refuse the unknown catalog {catalog:?}")
        });
        // A `ClientToken` the modelled `[!-~]{1,64}` pattern admits, one per row, so the
        // token never stands in the way of the catalog lookup under test.
        let requests =
            catalog_bound_requests(&catalog, UNKNOWN_BENEFIT, &format!("catalog-refusal-{index}"));
        for (operation, body) in &requests {
            let (status, response) = harness.call(operation, body).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got \
                 {response:?}"
            );
            let message = message_of(&response).unwrap_or_else(|| {
                panic!("case {case:?} on {operation}: the answer carries no message: {response:?}")
            });
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the helper's refusal {expected:?}, got \
                 {message:?}"
            );
        }
    }
}

/// The other side of the rule: for a catalog the service DOES hold the helper builds no
/// refusal, and every operation in the table gets past the catalog lookup and answers 200.
#[tokio::test]
async fn a_catalog_the_service_holds_is_refused_by_no_operation() {
    let harness = Harness::new();
    for (index, catalog) in SEEDED_CATALOGS.iter().enumerate() {
        let refusal = helper_refusal(&harness, catalog);
        assert!(
            refusal.is_none(),
            "case {catalog:?}: the helper must not refuse a catalog this service serves, got \
             {refusal:?}"
        );
        let benefit_id = text(catalog, &harness.active_benefit_of(catalog).await, "Id");
        let requests = catalog_bound_requests(
            catalog,
            &benefit_id,
            &format!("catalog-refusal-held-{index}"),
        );
        for (operation, body) in &requests {
            let (status, response) = harness.call(operation, body).await;
            assert_eq!(
                status, 200,
                "case {catalog:?} on {operation}: expected 200, got {status} with {response:?}"
            );
        }
    }
}
