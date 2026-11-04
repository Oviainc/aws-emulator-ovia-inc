// SPDX-License-Identifier: CC0-1.0

//! Where the catalog resolution sits relative to the partner-entitlement denial, over the
//! wire.
//!
//! The rule under test: an operation that WRITES resolves the `Catalog` its request names
//! before the account-level entitlement gate, so a catalog this service does not hold is
//! answered with that lookup's own 404 `ResourceNotFoundException` (``Catalog `<name>` not
//! found``) whatever the calling account holds, and the gate refuses only a request whose
//! catalog resolved; a REPORTING operation keeps the gate ahead of its lookups, so an
//! un-entitled caller reading the service learns nothing about which catalogs exist.
//!
//! The tables span the class the rule covers rather than one call: every operation that
//! binds a `Catalog` member, split into the writing and the reporting side (and the split
//! is asserted to cover every one of them, so a new catalog-bound operation cannot escape
//! the table), crossed with several catalog names the modelled `CatalogName` pattern
//! admits (`^[A-Za-z0-9_-]+$`, `smithy_query(query="shape",shape="CatalogName")`) and no
//! catalog answers to, with both entitlements, and — on the other side — every catalog the
//! service holds. The one bad-catalog create the issue reported is one row among them.
//!
//! Every assertion goes over the wire through the shared
//! [`crate::benefit_application_lifecycle::Harness`] (`MultiServiceRouter::dispatch`,
//! `awsJson1_0`, `POST /`, `X-Amz-Target: PartnerCentralBenefitsService.<Operation>`).

use serde_json::Value;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::SEEDED_CATALOGS;

use crate::benefit_application_lifecycle::{error_code, Harness};
use crate::catalog_resolution_one_accessor::store_snapshot;
use crate::service_error_rules::message_of;
use crate::unknown_catalog_lookup::{
    assert_catalog_not_found, catalog_bound_requests, unknown_catalogs, with_catalog,
};

/// The status this service serves its `AccessDeniedException` with, written out here
/// rather than read from the service's own constant so the value itself is under test. The
/// shape's modelled `@httpError` is 403 and the override is recorded behaviour, pinned on
/// its own in `tests/all/entitlement_denial.rs`.
const DENIED_STATUS: u16 = 400;

/// The sentence the entitlement gate refuses with, written out here rather than read from
/// the service's own constant so the text itself is under test. It carries no variable
/// part.
const ENTITLEMENT_MESSAGE: &str = "This action requires an active AWS partner benefit. \
Register as an AWS partner if you haven't already, or contact AWS partner central \
support.";

/// The catalog-bound operations that WRITE: each one mints, updates or retires a record
/// inside the catalog its request names (`smithy_query(query="digest")` for the surface,
/// and each operation's own documented effect for which side it is on).
const WRITING_OPERATIONS: &[&str] = &[
    "AmendBenefitApplication",
    "AssociateBenefitApplicationResource",
    "CancelBenefitApplication",
    "CreateBenefitApplication",
    "DisassociateBenefitApplicationResource",
    "RecallBenefitApplication",
    "SubmitBenefitApplication",
    "UpdateBenefitApplication",
];

/// The catalog-bound operations that only REPORT: a get or a listing of records the
/// catalog holds, which change nothing.
const REPORTING_OPERATIONS: &[&str] = &[
    "GetBenefit",
    "GetBenefitAllocation",
    "GetBenefitApplication",
    "ListBenefitAllocations",
    "ListBenefitApplications",
    "ListBenefits",
];

/// One well-formed request per catalog-bound operation, as the table drives it.
type Requests = Vec<(&'static str, Value)>;

/// The well-formed request per catalog-bound operation, split along the rule: the writing
/// side first, the reporting side second.
///
/// The split is checked against the whole set the sibling suite keeps, so an operation
/// that binds a `Catalog` and is named on neither list fails here instead of going
/// untested.
fn requests_by_side() -> (Requests, Requests) {
    let requests = catalog_bound_requests();
    let mut writing: Requests = Vec::new();
    let mut reporting: Requests = Vec::new();
    for (operation, body) in requests {
        if WRITING_OPERATIONS.contains(&operation) {
            writing.push((operation, body));
        } else if REPORTING_OPERATIONS.contains(&operation) {
            reporting.push((operation, body));
        } else {
            panic!(
                "case {operation:?}: a catalog-bound operation must be named on the writing or \
                 the reporting side of this suite"
            );
        }
    }
    let mut covered: Vec<&str> = writing
        .iter()
        .chain(reporting.iter())
        .map(|(operation, _)| *operation)
        .collect();
    covered.sort_unstable();
    let mut expected: Vec<&str> = WRITING_OPERATIONS
        .iter()
        .chain(REPORTING_OPERATIONS.iter())
        .copied()
        .collect();
    expected.sort_unstable();
    assert_eq!(
        covered, expected,
        "the two sides together must span every catalog-bound operation, got {covered:?}"
    );
    (writing, reporting)
}

/// Require the entitlement gate's own answer: [`DENIED_STATUS`], `AccessDeniedException`
/// and the fixed entitlement sentence.
async fn assert_denied(harness: &Harness, case: &str, operation: &str, body: &Value) {
    let (status, response) = harness.call(operation, body).await;
    assert_eq!(
        status, DENIED_STATUS,
        "case {case:?} on {operation}: expected {DENIED_STATUS}, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "AccessDeniedException",
        "case {case:?} on {operation}: expected AccessDeniedException, got {response:?}"
    );
    let message = message_of(&response).unwrap_or_default();
    assert_eq!(
        message, ENTITLEMENT_MESSAGE,
        "case {case:?} on {operation}: expected the entitlement sentence, got {message:?}"
    );
}

/// The rule's own table: on every WRITING operation, a catalog this service does not hold
/// is answered with the catalog lookup's 404 — the same answer, down to the message, for a
/// caller the entitlement gate would deny and for one it would admit, because the catalog
/// is resolved before the gate is consulted.
#[tokio::test]
async fn a_writing_operation_answers_an_unheld_catalog_ahead_of_the_entitlement_gate() {
    let unentitled = Harness::unentitled();
    let entitled = Harness::new();
    let (writing, _) = requests_by_side();
    for (name, catalog) in unknown_catalogs() {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog.as_str()),
            "case {name:?}: {catalog:?} must name no catalog this service holds"
        );
        for (operation, body) in &writing {
            let body = with_catalog(body, &catalog);
            // Both entitlements, pinned against the same expected answer, so the refusal
            // is the resource lookup's and not the account's.
            let case = format!("{name}, un-entitled");
            assert_catalog_not_found(&unentitled, &case, operation, &body).await;
            let case = format!("{name}, entitled");
            assert_catalog_not_found(&entitled, &case, operation, &body).await;
        }
    }
}

/// The other side of the gate's order: once the catalog resolves, a writing request from
/// an account with no active AWS partner benefit is still refused by the gate — on every
/// catalog this service holds, so the fix moved the catalog refusal ahead of the denial
/// without weakening it.
#[tokio::test]
async fn the_gate_still_refuses_a_writing_request_naming_a_catalog_this_service_holds() {
    let harness = Harness::unentitled();
    let (writing, _) = requests_by_side();
    for catalog in SEEDED_CATALOGS {
        for (operation, body) in &writing {
            let body = with_catalog(body, catalog);
            assert_denied(&harness, catalog, operation, &body).await;
        }
    }
}

/// The reporting side keeps the order it had: the denial answers an un-entitled caller
/// ahead of the catalog lookup, so the answer says nothing about which catalogs exist —
/// while the SAME request from an entitled caller reaches the lookup and gets its 404, so
/// the denial was a decision about the ACCOUNT and not a constant.
#[tokio::test]
async fn a_reporting_operation_answers_the_entitlement_denial_ahead_of_its_catalog_lookup() {
    let unentitled = Harness::unentitled();
    let entitled = Harness::new();
    let (_, reporting) = requests_by_side();
    for (name, catalog) in unknown_catalogs() {
        for (operation, body) in &reporting {
            let body = with_catalog(body, &catalog);
            assert_denied(&unentitled, name, operation, &body).await;
            let case = format!("{name}, entitled");
            assert_catalog_not_found(&entitled, &case, operation, &body).await;
        }
    }
}

/// Both refusals are reads of state: a request refused for an unheld catalog, and one
/// refused by the gate, leave every record this store holds exactly as it was — so the
/// reordering above moved no write ahead of either refusal.
#[tokio::test]
async fn a_refused_request_changes_nothing_whichever_refusal_answered_it() {
    let harness = Harness::unentitled();
    let before = store_snapshot(&harness);
    let (writing, reporting) = requests_by_side();
    for (name, catalog) in unknown_catalogs() {
        for (operation, body) in writing.iter().chain(reporting.iter()) {
            let body = with_catalog(body, &catalog);
            let (status, response) = harness.call(operation, &body).await;
            assert!(
                [404, DENIED_STATUS].contains(&status),
                "case {name:?} on {operation}: expected one of the two refusals, got {status} \
                 with {response:?}"
            );
        }
    }
    for catalog in SEEDED_CATALOGS {
        for (operation, body) in writing.iter().chain(reporting.iter()) {
            let body = with_catalog(body, catalog);
            assert_denied(&harness, catalog, operation, &body).await;
        }
    }
    let after = store_snapshot(&harness);
    assert_eq!(
        after, before,
        "a refused request must change no stored record"
    );
}
